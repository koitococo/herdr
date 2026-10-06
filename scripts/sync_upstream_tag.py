#!/usr/bin/env python3
"""Rebase a fork branch onto its newest eligible upstream tag and push it."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from dataclasses import dataclass


SYNC_REF_ROOT = "refs/upstream-tag-sync"
OID_PATTERN = re.compile(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}")


class SyncError(Exception):
    """An expected operational failure while synchronizing the branch."""


@dataclass
class GitResult:
    returncode: int
    stdout: str
    stderr: str


def git(args: list[str], *, label: str, check: bool = True) -> GitResult:
    result = subprocess.run(
        ["git", *args],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    completed = GitResult(result.returncode, result.stdout, result.stderr)
    if check and completed.returncode != 0:
        details = completed.stderr.strip() or completed.stdout.strip()
        suffix = f": {details}" if details else ""
        raise SyncError(f"{label} failed (git exited {completed.returncode}){suffix}")
    return completed


def require_clean_checkout() -> None:
    inside = git(["rev-parse", "--is-inside-work-tree"], label="Checking Git worktree")
    if inside.stdout.strip() != "true":
        raise SyncError("Run this command inside a non-bare Git checkout.")

    bare = git(["rev-parse", "--is-bare-repository"], label="Checking repository type")
    if bare.stdout.strip() != "false":
        raise SyncError("Run this command inside a non-bare Git checkout.")

    shallow = git(["rev-parse", "--is-shallow-repository"], label="Checking repository history")
    if shallow.stdout.strip() != "false":
        raise SyncError("A full-history checkout is required; shallow repositories are not supported.")

    status = git(
        ["status", "--porcelain", "--untracked-files=all"],
        label="Checking checkout cleanliness",
    )
    if status.stdout:
        raise SyncError("The checkout must be clean before upstream synchronization.")


def validate_branch(branch: str) -> None:
    if not branch or branch.startswith("-") or branch.startswith("refs/heads/"):
        raise SyncError(f"Invalid branch name: {branch!r}")
    result = git(["check-ref-format", "--branch", branch], label="Validating branch name", check=False)
    if result.returncode != 0 or result.stdout.strip() != branch:
        raise SyncError(f"Invalid branch name: {branch!r}")


def resolve_commit(revision: str, *, label: str) -> str:
    result = git(["rev-parse", "--verify", f"{revision}^{{commit}}"], label=label)
    oid = result.stdout.strip()
    if not OID_PATTERN.fullmatch(oid):
        raise SyncError(f"Git returned an invalid commit ID while {label.lower()}.")
    return oid.lower()


def newest_tag_after_base(upstream_ref: str, merge_base: str) -> tuple[str, str] | None:
    traversal = git(
        ["rev-list", "--topo-order", upstream_ref, f"^{merge_base}"],
        label="Walking upstream commits after the merge base",
    ).stdout.splitlines()
    if not traversal:
        return None

    tag_prefix = f"{SYNC_REF_ROOT}/tags/"
    refs = git(
        ["for-each-ref", "--format=%(refname)", f"{SYNC_REF_ROOT}/tags"],
        label="Listing fetched upstream tags",
    ).stdout.splitlines()
    tags_by_commit: dict[str, list[str]] = {}
    for ref in refs:
        if not ref.startswith(tag_prefix):
            raise SyncError(f"Unexpected ref in isolated upstream tag namespace: {ref}")
        tag_name = ref[len(tag_prefix) :]
        peeled = git(
            ["rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"],
            label=f"Resolving upstream tag {tag_name!r}",
            check=False,
        )
        if peeled.returncode == 1:
            # Tags to trees/blobs (or otherwise not commit-ish) are not candidates.
            continue
        if peeled.returncode != 0:
            details = peeled.stderr.strip() or peeled.stdout.strip()
            suffix = f": {details}" if details else ""
            raise SyncError(
                f"Resolving upstream tag {tag_name!r} failed (git exited {peeled.returncode}){suffix}"
            )
        commit = peeled.stdout.strip()
        if not OID_PATTERN.fullmatch(commit):
            raise SyncError(f"Git returned an invalid commit ID for upstream tag {tag_name!r}.")
        tags_by_commit.setdefault(commit.lower(), []).append(tag_name)

    # `rev-list upstream ^base` also includes merged side histories that forked
    # before base, so explicitly require base to be an ancestor of each candidate.
    # The traversal excludes base itself, making accepted candidates strict descendants.
    for commit in traversal:
        names = tags_by_commit.get(commit.lower())
        if not names:
            continue
        ancestry = git(
            ["merge-base", "--is-ancestor", merge_base, commit],
            label=f"Checking whether tag commit {commit.lower()} descends from the merge base",
            check=False,
        )
        if ancestry.returncode == 0:
            return commit.lower(), min(names)
        if ancestry.returncode != 1:
            details = ancestry.stderr.strip() or ancestry.stdout.strip()
            suffix = f": {details}" if details else ""
            raise SyncError(
                f"Checking candidate ancestry failed (git exited {ancestry.returncode}){suffix}"
            )
    return None


def synchronize(upstream_url: str, branch: str) -> None:
    validate_branch(branch)
    require_clean_checkout()

    git(["remote", "get-url", "origin"], label="Checking origin remote")

    origin_ref = f"refs/remotes/origin/{branch}"
    git(
        [
            "fetch",
            "--no-tags",
            "origin",
            f"+refs/heads/{branch}:{origin_ref}",
        ],
        label=f"Fetching origin branch {branch!r}",
    )
    original_origin_sha = resolve_commit(origin_ref, label="Resolving fetched origin branch")
    print(f"Fetched origin/{branch} at {original_origin_sha}.")

    # Fetch into a private namespace: upstream tags never overwrite local tags.
    upstream_branch_ref = f"{SYNC_REF_ROOT}/heads/{branch}"
    upstream_tags_ref = f"{SYNC_REF_ROOT}/tags/*"
    git(
        [
            "fetch",
            "--no-tags",
            "--prune",
            upstream_url,
            f"+refs/heads/{branch}:{upstream_branch_ref}",
            f"+refs/tags/*:{upstream_tags_ref}",
        ],
        label=f"Fetching upstream branch {branch!r} and tags",
    )
    upstream_sha = resolve_commit(upstream_branch_ref, label="Resolving fetched upstream branch")
    print(f"Fetched upstream/{branch} at {upstream_sha}.")

    merge = git(
        ["merge-base", original_origin_sha, upstream_sha],
        label="Finding the fork/upstream merge base",
        check=False,
    )
    if merge.returncode == 1:
        raise SyncError("The origin and upstream branches have unrelated histories; refusing to rebase.")
    if merge.returncode != 0:
        details = merge.stderr.strip() or merge.stdout.strip()
        suffix = f": {details}" if details else ""
        raise SyncError(f"Finding the fork/upstream merge base failed (git exited {merge.returncode}){suffix}")
    merge_base = merge.stdout.strip()
    if not OID_PATTERN.fullmatch(merge_base):
        raise SyncError("Git returned an invalid merge-base commit ID.")
    merge_base = merge_base.lower()
    print(f"Fork/upstream merge base: {merge_base}.")

    git(
        ["checkout", "-B", branch, original_origin_sha],
        label=f"Checking out {branch!r} at the fetched origin tip",
    )
    print(f"Checked out {branch} at the freshly fetched origin tip.")

    selection = newest_tag_after_base(upstream_branch_ref, merge_base)
    if selection is None:
        print("No upstream tag is a strict descendant of the merge base; no rebase or push needed.")
        return

    selected_sha, selected_name = selection
    print(f"Selected upstream tag {selected_name!r} at {selected_sha} by topological traversal order.")
    print(f"Rebasing {branch} onto {selected_name!r}...")
    rebase = git(
        ["rebase", "--onto", selected_sha, merge_base, branch],
        label="Rebasing fork commits onto the selected upstream tag",
        check=False,
    )
    if rebase.returncode != 0:
        details = rebase.stderr.strip() or rebase.stdout.strip()
        failure = f"Rebase failed (git exited {rebase.returncode})"
        if details:
            failure += f": {details}"
        aborted = git(["rebase", "--abort"], label="Aborting failed rebase", check=False)
        if aborted.returncode != 0:
            abort_details = aborted.stderr.strip() or aborted.stdout.strip()
            suffix = f": {abort_details}" if abort_details else ""
            raise SyncError(
                f"{failure}; git rebase --abort also failed (git exited {aborted.returncode}){suffix}; no push attempted."
            )
        raise SyncError(f"{failure}; rebase aborted; no push attempted.")

    push_ref = f"refs/heads/{branch}"
    git(
        [
            "push",
            f"--force-with-lease={push_ref}:{original_origin_sha}",
            "origin",
            f"HEAD:{push_ref}",
        ],
        label=f"Pushing rebased branch {branch!r} with an exact lease",
    )
    print(f"Pushed rebased {branch} to origin using a lease against {original_origin_sha}.")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-url", required=True, help="URL of the upstream Git repository")
    parser.add_argument("--branch", required=True, help="same-named origin/upstream branch to synchronize")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if not args.upstream_url:
        print("error: --upstream-url must not be empty", file=sys.stderr)
        return 2
    try:
        synchronize(args.upstream_url, args.branch)
    except SyncError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except OSError as error:
        print(f"error: unable to run Git: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
