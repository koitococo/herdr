import os
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SYNC_SCRIPT = Path(__file__).resolve().with_name("sync_upstream_tag.py")


class UpstreamTagSyncTests(unittest.TestCase):
    def setUp(self):
        self.env = os.environ.copy()
        self.env["GIT_CONFIG_GLOBAL"] = os.devnull
        self.env["GIT_CONFIG_NOSYSTEM"] = "1"
        self.temp = tempfile.TemporaryDirectory(prefix="herdr-upstream-sync-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.upstream = self.root / "upstream"
        self.origin = self.root / "origin.git"
        self.fork = self.root / "fork"

        self.upstream.mkdir()
        self.git(self.upstream, "init", "-q", "-b", "master")
        self.configure_identity(self.upstream)
        self.put(self.upstream, "root.txt", "shared root\n")
        self.commit(self.upstream, "shared root")
        self.common_ancestor = self.git(self.upstream, "rev-parse", "HEAD")
        self.put(self.upstream, "base.txt", "shared base\n")
        self.commit(self.upstream, "common base")
        self.merge_base = self.git(self.upstream, "rev-parse", "HEAD")

        self.git(self.root, "clone", "-q", "--bare", str(self.upstream), str(self.origin))
        self.git(self.root, "clone", "-q", str(self.origin), str(self.fork))
        self.configure_identity(self.fork)
        self.put(self.fork, "fork-only.txt", "fork change\n")
        self.commit(self.fork, "fork change")
        self.fork_tip = self.git(self.fork, "rev-parse", "HEAD")
        self.git(self.fork, "push", "-q", "origin", "HEAD:refs/heads/master")

    def git(self, cwd, *args, env=None):
        process_env = self.env.copy()
        if env:
            process_env.update(env)
        result = subprocess.run(
            ["git", *map(str, args)],
            cwd=cwd,
            env=process_env,
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode:
            raise AssertionError(
                f"git {' '.join(map(str, args))} failed in {cwd}:\n{result.stdout}{result.stderr}"
            )
        return result.stdout.strip()

    def configure_identity(self, repo):
        self.git(repo, "config", "user.name", "Upstream Sync Test")
        self.git(repo, "config", "user.email", "upstream-sync@example.invalid")
        self.git(repo, "config", "commit.gpgsign", "false")

    def put(self, repo, path, contents):
        target = Path(repo) / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(contents, encoding="utf-8")

    def commit(self, repo, message):
        self.git(repo, "add", "--all")
        self.git(repo, "commit", "-qm", message)
        return self.git(repo, "rev-parse", "HEAD")

    def upstream_commit(self, path, contents, message=None):
        self.put(self.upstream, path, contents)
        return self.commit(self.upstream, message or path)

    def upstream_tag(self, name, commit, annotated=False, tagger_date=None):
        args = ["tag"]
        if annotated:
            args.extend(["-a", name, "-m", name])
        else:
            args.append(name)
        args.append(commit)
        env = {"GIT_COMMITTER_DATE": tagger_date} if tagger_date else None
        self.git(self.upstream, *args, env=env)

    def run_sync(self, upstream=None, env=None):
        process_env = self.env.copy()
        if env:
            process_env.update(env)
        return subprocess.run(
            [
                sys.executable,
                str(SYNC_SCRIPT),
                "--upstream-url",
                str(upstream or self.upstream),
                "--branch",
                "master",
            ],
            cwd=self.fork,
            env=process_env,
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )

    def head(self, repo=None):
        return self.git(repo or self.fork, "rev-parse", "HEAD")

    def origin_tip(self):
        return self.git(self.origin, "rev-parse", "refs/heads/master")

    def is_ancestor(self, ancestor, descendant, repo=None):
        result = subprocess.run(
            ["git", "merge-base", "--is-ancestor", ancestor, descendant],
            cwd=repo or self.fork,
            env=self.env,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertIn(result.returncode, (0, 1), result.stderr)
        return result.returncode == 0

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def assert_no_op(self, result, original_head):
        self.assert_success(result)
        self.assertEqual(self.head(), original_head)
        self.assertEqual(self.origin_tip(), original_head)

    def assert_failed_without_push(self, result, original_head):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.head(), original_head)
        self.assertEqual(self.origin_tip(), original_head)

    def test_rebases_to_latest_tag_before_untagged_head_and_is_idempotent(self):
        older = self.upstream_commit("tagged-older.txt", "older\n")
        self.upstream_tag("v1-annotated", older, annotated=True)
        latest = self.upstream_commit("tagged-latest.txt", "latest\n")
        self.upstream_tag("v2-lightweight", latest)
        untagged_head = self.upstream_commit("untagged-head.txt", "must not be included\n")

        result = self.run_sync()
        self.assert_success(result)
        synced_head = self.head()
        self.assertNotEqual(synced_head, self.fork_tip)
        self.assertTrue(self.is_ancestor(latest, synced_head))
        self.assertFalse(self.is_ancestor(untagged_head, synced_head))
        self.assertEqual((self.fork / "fork-only.txt").read_text(encoding="utf-8"), "fork change\n")
        self.assertEqual((self.fork / "tagged-latest.txt").read_text(encoding="utf-8"), "latest\n")
        self.assertFalse((self.fork / "untagged-head.txt").exists())
        self.assertEqual(self.origin_tip(), synced_head)

        rerun = self.run_sync()
        self.assert_success(rerun)
        self.assertEqual(self.head(), synced_head)
        self.assertEqual(self.origin_tip(), synced_head)

    def test_tag_names_and_tagger_timestamps_do_not_override_ancestry_order(self):
        earlier = self.upstream_commit("earlier-tag.txt", "earlier\n")
        self.upstream_tag(
            "v99.0.0",
            earlier,
            annotated=True,
            tagger_date="2099-01-01T00:00:00+0000",
        )
        later = self.upstream_commit("later-tag.txt", "later\n")
        self.upstream_tag(
            "v1.0.0",
            later,
            annotated=True,
            tagger_date="2000-01-01T00:00:00+0000",
        )
        self.upstream_tag("z-tie", later)
        self.upstream_tag("a-tie", later)
        untagged_head = self.upstream_commit("after-tags.txt", "after tags\n")

        result = self.run_sync()
        self.assert_success(result)

        synced_head = self.head()
        self.assertTrue(self.is_ancestor(later, synced_head))
        self.assertFalse(self.is_ancestor(untagged_head, synced_head))
        self.assertEqual((self.fork / "later-tag.txt").read_text(encoding="utf-8"), "later\n")
        self.assertFalse((self.fork / "after-tags.txt").exists())
        self.assertEqual(self.origin_tip(), synced_head)

    def test_no_tags_and_only_ineligible_tags_are_successful_no_ops(self):
        untagged = self.upstream_commit("untagged.txt", "upstream\n")
        original_head = self.fork_tip
        self.assert_no_op(self.run_sync(), original_head)
        self.assertNotEqual(untagged, self.merge_base)

        self.upstream_tag("at-merge-base", self.merge_base)
        self.upstream_tag("before-merge-base", self.common_ancestor, annotated=True)
        self.git(self.upstream, "checkout", "-q", "-b", "off-branch", self.merge_base)
        off_branch = self.upstream_commit("off-branch.txt", "not on master\n")
        self.upstream_tag("off-branch-tag", off_branch)
        self.git(self.upstream, "checkout", "-q", "master")
        self.assertFalse(self.is_ancestor(off_branch, self.git(self.upstream, "rev-parse", "master"), self.upstream))

        self.assert_no_op(self.run_sync(), original_head)

    def test_tagged_pre_merge_base_side_commit_is_not_eligible_after_merge(self):
        self.git(self.upstream, "checkout", "-q", "-b", "pre-merge-base-side", self.common_ancestor)
        side_commit = self.upstream_commit("tagged-side-history.txt", "side history\n")
        self.upstream_tag("tagged-side-history", side_commit)
        self.git(self.upstream, "checkout", "-q", "master")
        self.git(self.upstream, "merge", "-q", "--no-edit", "pre-merge-base-side")

        upstream_tip = self.git(self.upstream, "rev-parse", "master")
        self.assertTrue(self.is_ancestor(side_commit, upstream_tip, self.upstream))
        self.assertFalse(self.is_ancestor(self.merge_base, side_commit, self.upstream))
        result = self.run_sync()
        self.assert_no_op(result, self.fork_tip)
        self.assertEqual(
            self.git(self.fork, "merge-base", self.fork_tip, upstream_tip),
            self.merge_base,
        )

    def test_rebase_conflict_aborts_and_does_not_push(self):
        self.put(self.fork, "conflict.txt", "fork side\n")
        self.commit(self.fork, "fork conflict")
        original_head = self.head()
        self.git(self.fork, "push", "-q", "origin", "HEAD:refs/heads/master")

        conflicting_upstream = self.upstream_commit("conflict.txt", "upstream side\n")
        self.upstream_tag("conflicting-tag", conflicting_upstream)
        result = self.run_sync()

        self.assert_failed_without_push(result, original_head)
        self.assertEqual(self.git(self.fork, "status", "--porcelain"), "")

    def test_upstream_tag_with_same_name_does_not_overwrite_fork_tag(self):
        tagged = self.upstream_commit("upstream-release.txt", "upstream release\n")
        self.upstream_tag("release", tagged, annotated=True)
        self.upstream_commit("untagged-after-release.txt", "later upstream head\n")
        self.git(self.fork, "tag", "release", self.fork_tip)
        local_tag_before = self.git(self.fork, "rev-parse", "refs/tags/release")
        self.assertEqual(local_tag_before, self.fork_tip)

        result = self.run_sync()
        self.assert_success(result)
        synced_head = self.head()
        self.assertTrue(self.is_ancestor(tagged, synced_head))
        self.assertFalse((self.fork / "untagged-after-release.txt").exists())
        self.assertEqual(self.git(self.fork, "rev-parse", "refs/tags/release"), local_tag_before)
        self.assertEqual(self.origin_tip(), synced_head)

    @unittest.skipIf(os.name == "nt", "the deterministic Git PATH race shim uses a POSIX shell")
    def test_explicit_lease_rejects_origin_advancement_before_push(self):
        tagged = self.upstream_commit("eligible.txt", "eligible\n")
        self.upstream_tag("eligible-tag", tagged)

        racer = self.root / "racer"
        self.git(self.root, "clone", "-q", str(self.origin), str(racer))
        self.configure_identity(racer)
        self.put(racer, "concurrent.txt", "concurrent origin update\n")
        concurrent_tip = self.commit(racer, "concurrent origin update")
        self.git(racer, "push", "-q", "origin", "HEAD:refs/heads/race-candidate")

        real_git = shutil.which("git")
        self.assertIsNotNone(real_git)
        race_bin = self.root / "race-bin"
        race_bin.mkdir()
        marker = self.root / "origin-advanced"
        wrapper = race_bin / "git"
        wrapper.write_text(
            "#!/bin/sh\n"
            "set -eu\n"
            "for arg in \"$@\"; do\n"
            f"  if [ \"$arg\" = push ] && [ ! -e {shlex.quote(str(marker))} ]; then\n"
            f"    : > {shlex.quote(str(marker))}\n"
            f"    {shlex.quote(real_git)} {shlex.quote('--git-dir=' + str(self.origin))} "
            f"update-ref refs/heads/master {shlex.quote(concurrent_tip)} {shlex.quote(self.fork_tip)}\n"
            "    break\n"
            "  fi\n"
            "done\n"
            f"exec {shlex.quote(real_git)} \"$@\"\n",
            encoding="utf-8",
        )
        wrapper.chmod(wrapper.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

        path = os.pathsep.join([str(race_bin), self.env.get("PATH", "")])
        result = self.run_sync(env={"PATH": path, "LC_ALL": "C"})
        self.assertTrue(marker.exists(), "the deterministic race did not advance origin before push")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(self.head(), concurrent_tip, "the raced origin commit must not become the sync head")
        self.assertEqual(self.origin_tip(), concurrent_tip)

    def test_missing_upstream_branch_fails_without_push(self):
        original_head = self.fork_tip
        self.git(self.upstream, "branch", "-m", "upstream-only")

        result = self.run_sync()

        self.assert_failed_without_push(result, original_head)

    def test_unrelated_upstream_history_fails_without_push(self):
        unrelated = self.root / "unrelated-upstream"
        unrelated.mkdir()
        self.git(unrelated, "init", "-q", "-b", "master")
        self.configure_identity(unrelated)
        self.put(unrelated, "independent.txt", "unrelated history\n")
        self.commit(unrelated, "unrelated root")
        original_head = self.fork_tip

        result = self.run_sync(upstream=unrelated)

        self.assert_failed_without_push(result, original_head)


if __name__ == "__main__":
    unittest.main()
