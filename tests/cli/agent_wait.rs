use super::harness::*;

#[test]
fn agent_wait_exits_immediately_when_status_already_matches() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_immediate_1","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pane_id = format!("{workspace_id}:p1");

    let reported = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_immediate_2","method":"pane.report_agent","params":{{"pane_id":"{}","source":"custom:test","agent":"pi","state":"idle"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(reported["result"]["type"], "ok");

    let waited = run_cli(
        &socket_path,
        &[
            "agent",
            "wait",
            &pane_id,
            "--until",
            "idle",
            "--timeout",
            "1000",
        ],
    );
    assert!(
        waited.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&waited.stderr)
    );
    let waited_json: serde_json::Value = serde_json::from_slice(&waited.stdout).unwrap();
    assert_eq!(waited_json["result"]["agent"]["agent_status"], "idle");
    assert_eq!(waited_json["result"]["agent"]["agent"], "pi");

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn agent_wait_times_out_when_status_does_not_match() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_timeout_1","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    assert_eq!(created["result"]["type"], "workspace_created");
    let pane_id = created["result"]["root_pane"]["pane_id"].as_str().unwrap();
    let reported = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_timeout_2","method":"pane.report_agent","params":{{"pane_id":"{}","source":"custom:test","agent":"pi","state":"working"}}}}"#,
            pane_id
        ),
    );
    assert_eq!(reported["result"]["type"], "ok");

    let waited = run_cli(
        &socket_path,
        &[
            "agent",
            "wait",
            pane_id,
            "--until",
            "blocked",
            "--timeout",
            "100",
        ],
    );
    assert!(!waited.status.success());
    assert!(
        String::from_utf8_lossy(&waited.stderr).contains("timed out waiting for agent status"),
        "stderr: {}",
        String::from_utf8_lossy(&waited.stderr)
    );

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn agent_wait_exits_when_done_status_matches() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_status_1","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let background = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_status_background","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    assert_ne!(
        background["result"]["workspace"]["workspace_id"],
        created["result"]["workspace"]["workspace_id"]
    );

    for state in ["working", "idle"] {
        let reported = send_request(
            &socket_path,
            &format!(
                r#"{{"id":"req_cli_status_{state}","method":"pane.report_agent","params":{{"pane_id":"{pane_id}","source":"custom:test","agent":"pi","state":"{state}"}}}}"#
            ),
        );
        assert_eq!(reported["result"]["type"], "ok");
    }

    let waited = run_cli(
        &socket_path,
        &[
            "agent",
            "wait",
            &pane_id,
            "--until",
            "done",
            "--timeout",
            "10000",
        ],
    );
    assert!(
        waited.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&waited.stderr)
    );
    let waited_json: serde_json::Value = serde_json::from_slice(&waited.stdout).unwrap();
    assert_eq!(waited_json["result"]["agent"]["agent_status"], "done");
    assert_eq!(waited_json["result"]["agent"]["agent"], "pi");

    cleanup_spawned_herdr(herdr, base);
}
