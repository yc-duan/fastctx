//! Control-center runtime contracts.
//!
//! The whole suite rests on the `FASTCTX_TEST_*` runtime hooks — the idle-timeout override, the
//! build-id override that isolates endpoints, and the host start event log. All three are
//! `debug_assertions`-only, so in a release build every test here would probe a control center it
//! can neither isolate nor observe. The contracts are opt-level independent, so they are covered
//! once, in debug, rather than by promoting the hooks to production environment switches.
#![cfg(debug_assertions)]

mod common;

use common::{McpSession, mcp_text, normalized};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

// Every use is a positive poll-until wait, so widening only delays failure reports. Some waits
// cover real command runtime: pushing 2 MiB through the Git Bash pipe plus supervisor draining
// measures 15-18 s on a loaded Windows host, which sat exactly on the previous 15 s deadline.
const PROCESS_DEADLINE: Duration = Duration::from_secs(45);

#[test]
fn unavailable_control_center_falls_back_before_stdin_is_consumed_and_reports_it() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("visible.txt"), "fallback\n").unwrap();
    let event_log = temp.path().join("runtime-events.log");
    #[cfg(windows)]
    let blocked_runtime = temp.path().join("runtime-is-a-file");
    #[cfg(unix)]
    // Keep this below sun_path limits so the test reaches the blocked-runtime failure. (2026-08-02)
    let blocked_runtime = tempfile::Builder::new()
        .prefix("fctx-blocked-")
        .tempfile_in("/tmp")
        .unwrap();
    #[cfg(windows)]
    std::fs::write(&blocked_runtime, "blocked").unwrap();
    let mut command = server_command(&home, &workspace, &event_log);
    #[cfg(windows)]
    command.env("LOCALAPPDATA", &blocked_runtime);
    #[cfg(unix)]
    command.env("XDG_RUNTIME_DIR", blocked_runtime.path());

    let mut session = McpSession::start(command);
    let response = session.call("glob", serde_json::json!({"pattern": "**/*.txt"}));
    assert!(mcp_text(&response).contains("visible.txt"), "{response}");
    let (_, stderr) = session.kill_proxy_with_stderr();
    assert!(
        stderr.contains("falling back to a full standalone MCP server"),
        "{stderr}"
    );
    assert!(host_start_pids(&event_log).is_empty());
}

#[test]
fn one_hundred_cold_proxies_start_exactly_one_control_center() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let event_log = temp.path().join("runtime-events.log");

    let mut proxies = Vec::new();
    for _ in 0..100 {
        let mut command = server_command(&home, &workspace, &event_log);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        proxies.push(command.spawn().unwrap());
    }

    let hosts = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE);
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        host_start_pids(&event_log),
        hosts,
        "more than one host started"
    );
    assert_eq!(
        proxies
            .iter_mut()
            .map(|proxy| proxy.try_wait().unwrap().is_none())
            .filter(|running| *running)
            .count(),
        100,
        "every thin proxy must remain connected"
    );

    for mut proxy in proxies {
        let _ = proxy.kill();
        let _ = proxy.wait();
    }
    terminate_process(hosts[0]);
}

#[test]
fn connection_context_keeps_cwd_path_budget_cursor_and_cancellation_isolated() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let first_bin = first.join("bin");
    let second_bin = second.join("bin");
    for directory in [&first_bin, &second_bin] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(first.join("cwd.txt"), "cwd-first\n").unwrap();
    std::fs::write(second.join("cwd.txt"), "cwd-second\n").unwrap();
    write_path_command(&first_bin, "path-first");
    write_path_command(&second_bin, "path-second");
    let event_log = temp.path().join("runtime-events.log");

    let mut first_command = server_command(&home, &first, &event_log);
    prepend_path(&mut first_command, &first_bin);
    first_command
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", "300")
        .env("SESSION_VALUE", "env-first")
        .env("FASTCTX_TOKEN_BUDGET", "1000")
        .env("FASTCTX_RUN_TOKEN_BUDGET", "1000");
    let mut first_session = McpSession::start(first_command);

    let mut second_command = server_command(&home, &second, &event_log);
    prepend_path(&mut second_command, &second_bin);
    second_command
        .env("SESSION_VALUE", "env-second")
        .env("FASTCTX_TOKEN_BUDGET", "1000")
        .env("FASTCTX_RUN_TOKEN_BUDGET", "1000");
    let mut second_session = McpSession::start(second_command);

    for (session, expected) in [
        (&mut first_session, "cwd-first\npath-first\nenv-first"),
        (&mut second_session, "cwd-second\npath-second\nenv-second"),
    ] {
        let response = session.call(
            "run",
            serde_json::json!({
                "command": "cat cwd.txt; session-value; printf '%s' \"$SESSION_VALUE\"",
                "login_shell": false
            }),
        );
        assert!(mcp_text(&response).starts_with(expected), "{response}");
    }

    let mut invalid_budget_command = server_command(&home, &first, &event_log);
    invalid_budget_command
        .env("FASTCTX_TOKEN_BUDGET", "1000")
        .env("FASTCTX_RUN_TOKEN_BUDGET", "2000");
    let mut invalid_budget = McpSession::start(invalid_budget_command);
    let rejected = invalid_budget.call(
        "run",
        serde_json::json!({"command": "printf must-not-run", "login_shell": false}),
    );
    assert_eq!(rejected["result"]["isError"], true);
    assert!(mcp_text(&rejected).contains("exceeds FASTCTX_TOKEN_BUDGET=1000"));
    let healthy = second_session.call(
        "run",
        serde_json::json!({"command": "printf budget-second", "login_shell": false}),
    );
    assert!(mcp_text(&healthy).starts_with("budget-second"));

    let started = second_session.call(
        "run_background",
        serde_json::json!({
            "command": "printf 'cursor-one\\ncursor-two\\n'",
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&started));
    wait_for_file(
        &home.join(".fastctx/jobs").join(&job_id).join("exit.json"),
        PROCESS_DEADLINE,
    );
    let first_cursor = first_session.call(
        "job_output",
        serde_json::json!({"job_id": &job_id, "wait_ms": 0}),
    );
    let second_cursor = second_session.call(
        "job_output",
        serde_json::json!({"job_id": &job_id, "wait_ms": 0}),
    );
    for response in [first_cursor, second_cursor] {
        let text = mcp_text(&response);
        assert!(text.contains("cursor-one"), "{text}");
        assert!(text.contains("cursor-two"), "{text}");
    }

    let started_marker = first.join("foreground-started");
    let escaped_marker = first.join("foreground-escaped");
    let command = format!(
        "printf started > {}; sleep 30; printf escaped > {}",
        shell_quote(&normalized(&started_marker)),
        shell_quote(&normalized(&escaped_marker))
    );
    first_session.begin_call(
        "run",
        serde_json::json!({"command": command, "login_shell": false, "timeout_ms": 60000}),
    );
    wait_for_file(&started_marker, PROCESS_DEADLINE);
    kill_proxy_tree(first_session.child_id());
    let _ = first_session.kill_proxy();
    let survivor = second_session.call(
        "run",
        serde_json::json!({"command": "printf survivor", "login_shell": false}),
    );
    assert!(mcp_text(&survivor).starts_with("survivor"));
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !escaped_marker.exists(),
        "cancelled session A escaped its owner"
    );

    let hosts = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE);
    assert_eq!(hosts.len(), 1);
    let _ = invalid_budget.kill_proxy();
    let _ = second_session.kill_proxy();
    wait_for_process_exit(hosts[0], PROCESS_DEADLINE);
}

#[test]
fn runtime_guard_tightens_without_apply_and_releases_on_the_next_connection() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    let codex = home.join(".codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        codex.join("config.toml"),
        "model_provider='third'\n[model_providers.third]\nname='Third Party'\n",
    )
    .unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let output_command = "yes guarded-word | head -n 14000";

    let mut guarded_command = server_command(&home, &workspace, &event_log);
    guarded_command
        .env("FASTCTX_TOKEN_BUDGET", "54000")
        .env("FASTCTX_RUN_TOKEN_BUDGET", "54000");
    let mut guarded = McpSession::start(guarded_command);
    let inherited = guarded.call(
        "run",
        serde_json::json!({
            "command": "printf '%s/%s' \"$FASTCTX_TOKEN_BUDGET\" \"$FASTCTX_RUN_TOKEN_BUDGET\"",
            "login_shell": false
        }),
    );
    assert!(mcp_text(&inherited).starts_with("54000/54000"));
    let guarded_response = guarded.call(
        "run",
        serde_json::json!({"command": output_command, "login_shell": false}),
    );
    let guarded_text = mcp_text(&guarded_response);
    let tokenizer = tiktoken_rs::o200k_base_singleton();
    let guarded_tokens = tokenizer.encode_ordinary(guarded_text).len();
    assert!(
        guarded_tokens <= 9_000,
        "guarded response used {guarded_tokens} tokens"
    );
    assert!(guarded_text.contains("(Partial:"), "{guarded_text}");

    std::fs::create_dir_all(home.join(".fastctx")).unwrap();
    std::fs::write(
        home.join(".fastctx/config.toml"),
        "schema_version = 1\n\n[output_guard]\nenabled = false\n",
    )
    .unwrap();
    let mut unguarded_command = server_command(&home, &workspace, &event_log);
    unguarded_command
        .env("FASTCTX_TOKEN_BUDGET", "54000")
        .env("FASTCTX_RUN_TOKEN_BUDGET", "54000");
    let mut unguarded = McpSession::start(unguarded_command);
    let unguarded_response = unguarded.call(
        "run",
        serde_json::json!({"command": output_command, "login_shell": false}),
    );
    let unguarded_tokens = tokenizer
        .encode_ordinary(mcp_text(&unguarded_response))
        .len();
    assert!(
        unguarded_tokens > guarded_tokens + 2_000,
        "guarded={guarded_tokens}, unguarded={unguarded_tokens}"
    );
    assert_eq!(host_start_pids(&event_log).len(), 1);

    let _ = guarded.kill_proxy();
    let _ = unguarded.kill_proxy();
    terminate_process(host_start_pids(&event_log)[0]);
}

#[test]
fn output_quota_stops_log_and_index_growth_but_not_the_command_or_exit_status() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(home.join(".fastctx")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        home.join(".fastctx/config.toml"),
        "schema_version = 1\n\n[fastshell]\nenabled = true\njob_storage_limit_mib = 1\n",
    )
    .unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let marker = workspace.join("command-finished");
    let mut session = McpSession::start(server_command(&home, &workspace, &event_log));
    let response = session.call(
        "run_background",
        serde_json::json!({
            "command": format!(
                "yes x | head -c 2097152; sleep 1; printf done > {}; exit 17",
                shell_quote(&normalized(&marker))
            ),
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&response));
    let job_dir = home.join(".fastctx/jobs").join(&job_id);
    wait_for_file(&job_dir.join("output-truncated.json"), PROCESS_DEADLINE);
    let running_output = session.call(
        "job_output",
        serde_json::json!({"job_id": &job_id, "wait_ms": 0, "after_seq": 0}),
    );
    let running_output = mcp_text(&running_output);
    assert!(running_output.contains("running"), "{running_output}");
    assert!(
        running_output.contains("combined output.log + output.idx hard limit"),
        "{running_output}"
    );
    wait_for_file(&job_dir.join("exit.json"), PROCESS_DEADLINE);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "done");

    let output = session.call(
        "job_output",
        serde_json::json!({"job_id": &job_id, "wait_ms": 0, "after_seq": 0}),
    );
    let output = mcp_text(&output);
    assert!(
        output.contains("combined output.log + output.idx hard limit"),
        "{output}"
    );
    assert!(output.contains("kept draining output"), "{output}");
    assert!(output.contains("exited 17"), "{output}");
    let combined = std::fs::metadata(job_dir.join("output.log")).unwrap().len()
        + std::fs::metadata(job_dir.join("output.idx")).unwrap().len();
    assert!(
        combined <= 1024 * 1024,
        "combined output grew to {combined}"
    );
    assert!(job_dir.join("output-truncated.json").is_file());
    let exit: serde_json::Value =
        serde_json::from_slice(&std::fs::read(job_dir.join("exit.json")).unwrap()).unwrap();
    assert_eq!(exit["exit_code"], 17);
    assert!(exit["output_truncation"].is_object());
    let truncation = &exit["output_truncation"];
    assert_eq!(truncation["limit_bytes"], 1024 * 1024);
    assert_eq!(
        truncation["persisted_log_bytes"].as_u64().unwrap()
            + truncation["persisted_index_bytes"].as_u64().unwrap(),
        combined
    );
    assert_eq!(truncation["after_seq"], exit["total_lines"]);

    let _ = session.kill_proxy();
    terminate_process(host_start_pids(&event_log)[0]);
}

#[test]
fn periodic_maintenance_reaps_finished_history_without_a_new_job_start() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(home.join(".fastctx")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        home.join(".fastctx/config.toml"),
        "schema_version = 1\n\n[fastshell]\nenabled = true\njob_storage_limit_mib = 1\n",
    )
    .unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let mut command = server_command(&home, &workspace, &event_log);
    command.env("FASTCTX_TEST_RUNTIME_MAINTENANCE_MS", "100");
    let mut session = McpSession::start(command);
    let response = session.call(
        "run_background",
        serde_json::json!({"command": "printf finished", "login_shell": false}),
    );
    let job_id = started_job_id(mcp_text(&response));
    let job_dir = home.join(".fastctx/jobs").join(job_id);
    wait_for_file(&job_dir.join("exit.json"), PROCESS_DEADLINE);
    std::fs::write(
        job_dir.join("post-finish-payload"),
        vec![b'x'; 2 * 1024 * 1024],
    )
    .unwrap();
    wait_for_path_absence(&job_dir, PROCESS_DEADLINE);

    let _ = session.kill_proxy();
    terminate_process(host_start_pids(&event_log)[0]);
}

#[test]
fn build_id_isolation_keeps_old_and_new_control_centers_independent() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let event_log = temp.path().join("runtime-events.log");

    let mut old_command = server_command(&home, &workspace, &event_log);
    old_command.env("FASTCTX_TEST_BUILD_ID", "build-old");
    let mut old = McpSession::start(old_command);
    let mut new_command = server_command(&home, &workspace, &event_log);
    new_command.env("FASTCTX_TEST_BUILD_ID", "build-new");
    let mut new = McpSession::start(new_command);
    let hosts = wait_for_host_starts(&event_log, 2, PROCESS_DEADLINE);
    assert_ne!(hosts[0], hosts[1]);

    let old_response = old.call(
        "run",
        serde_json::json!({"command": "printf old", "login_shell": false}),
    );
    let new_response = new.call(
        "run",
        serde_json::json!({"command": "printf new", "login_shell": false}),
    );
    assert!(mcp_text(&old_response).starts_with("old"));
    assert!(mcp_text(&new_response).starts_with("new"));
    terminate_process(hosts[0]);
    let survivor = new.call(
        "run",
        serde_json::json!({"command": "printf still-new", "login_shell": false}),
    );
    assert!(mcp_text(&survivor).starts_with("still-new"));

    let _ = old.kill_proxy();
    let _ = new.kill_proxy();
    terminate_process(hosts[1]);
}

#[test]
fn a_control_center_crash_keeps_the_session_alive_without_replaying_an_inflight_request() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let counter = workspace.join("side-effect-count");
    let command_text = format!(
        "value=$(cat {} 2>/dev/null || printf 0); expr \"$value\" + 1 > {}; sleep 2",
        shell_quote(&normalized(&counter)),
        shell_quote(&normalized(&counter))
    );
    let mut session = McpSession::start(server_command(&home, &workspace, &event_log));
    let inflight = session.begin_call(
        "run",
        serde_json::json!({
            "command": &command_text,
            "login_shell": false,
            "timeout_ms": 60_000
        }),
    );
    wait_for_text(&counter, "1", PROCESS_DEADLINE);
    let first_host = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE)[0];

    terminate_process(first_host);
    wait_for_process_exit(first_host, PROCESS_DEADLINE);

    // The host never restarts a stdio MCP server it lost, so the session has to outlive its engine.
    let answer = session.await_response_with_timeout(inflight, PROCESS_DEADLINE);
    let failure = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(
        failure.contains("control center"),
        "an interrupted call must be closed out explicitly: {answer}"
    );
    let hosts = wait_for_host_starts(&event_log, 2, PROCESS_DEADLINE);
    assert_ne!(hosts[0], hosts[1]);
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().trim(),
        "1",
        "the proxy must not replay a completed side-effecting request"
    );
    let healthy = session.call_with_timeout(
        "run",
        serde_json::json!({"command": "printf rebuilt", "login_shell": false}),
        PROCESS_DEADLINE,
    );
    assert!(mcp_text(&healthy).starts_with("rebuilt"), "{healthy}");

    let status = session.close();
    assert!(status.success(), "stdin EOF must remain a clean proxy exit");
    terminate_process(hosts[1]);
}

#[test]
fn live_connection_survives_idle_and_host_exits_only_after_disconnect() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let mut command = server_command(&home, &workspace, &event_log);
    // The contract is a fresh idle window, not a 300 ms proxy-close benchmark. (2026-08-02)
    command.env("FASTCTX_TEST_RUNTIME_IDLE_MS", "1000");
    let mut session = McpSession::start(command);
    let host = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE)[0];
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        process_is_alive(host),
        "a live connection must suppress idle shutdown"
    );
    let resumed = session.call(
        "run",
        serde_json::json!({"command": "printf resumed", "login_shell": false}),
    );
    assert!(mcp_text(&resumed).starts_with("resumed"), "{resumed}");

    let status = session.close();
    assert!(status.success(), "stdin EOF must remain a clean proxy exit");
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        process_is_alive(host),
        "idle time must restart when the final connection disconnects"
    );
    wait_for_process_exit(host, PROCESS_DEADLINE);
}

#[test]
fn running_job_delays_zero_connection_idle_exit() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let mut command = server_command(&home, &workspace, &event_log);
    command.env("FASTCTX_TEST_RUNTIME_IDLE_MS", "300");
    let mut session = McpSession::start(command);
    let response = session.call(
        "run_background",
        serde_json::json!({"command": "sleep 3; exit 0", "login_shell": false}),
    );
    let job_id = started_job_id(mcp_text(&response));
    let job_exit = home.join(".fastctx/jobs").join(job_id).join("exit.json");
    let host = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE)[0];
    assert!(session.close().success());

    // The job outlives the observation window on purpose: without the running-job check the host
    // reaches zero connections and exits roughly one idle period after close, well before 1.2 s.
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        process_is_alive(host),
        "a running job must suppress zero-connection idle shutdown"
    );
    wait_for_file(&job_exit, PROCESS_DEADLINE);
    wait_for_process_exit(host, PROCESS_DEADLINE);
}

#[test]
fn a_damaged_job_record_does_not_pin_the_control_center_open() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let damaged = home.join(".fastctx/jobs/j-abc123");
    std::fs::create_dir_all(&damaged).unwrap();
    std::fs::write(damaged.join("meta.json"), "not json {").unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let mut command = server_command(&home, &workspace, &event_log);
    command.env("FASTCTX_TEST_RUNTIME_IDLE_MS", "300");
    let mut session = McpSession::start(command);
    let response = session.call(
        "run",
        serde_json::json!({"command": "printf ready", "login_shell": false}),
    );
    assert!(mcp_text(&response).starts_with("ready"), "{response}");
    let host = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE)[0];
    assert!(session.close().success());

    // Guards the fail-open exit: the damaged record fails every registry scan, so a host that
    // insisted on a clean scan before exiting would sit here forever as a zero-connection zombie.
    wait_for_process_exit(host, PROCESS_DEADLINE);
    assert!(
        damaged.join("meta.json").is_file(),
        "idle shutdown must not silently repair or remove the damaged record"
    );
}

#[test]
fn multiple_idle_connections_resume_together_through_the_same_host() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let mut first_command = server_command(&home, &workspace, &event_log);
    first_command.env("FASTCTX_TEST_RUNTIME_IDLE_MS", "300");
    let mut second_command = server_command(&home, &workspace, &event_log);
    second_command.env("FASTCTX_TEST_RUNTIME_IDLE_MS", "300");
    let mut first = McpSession::start(first_command);
    let mut second = McpSession::start(second_command);
    let host = wait_for_host_starts(&event_log, 1, PROCESS_DEADLINE)[0];

    std::thread::sleep(Duration::from_millis(900));
    assert!(process_is_alive(host), "live idle sessions lost their host");
    let first_id = first.begin_call(
        "run",
        serde_json::json!({"command": "printf first", "login_shell": false}),
    );
    let second_id = second.begin_call(
        "run",
        serde_json::json!({"command": "printf second", "login_shell": false}),
    );
    let first_response = first.await_response(first_id);
    let second_response = second.await_response(second_id);
    assert!(mcp_text(&first_response).starts_with("first"));
    assert!(mcp_text(&second_response).starts_with("second"));
    assert_eq!(
        host_start_pids(&event_log),
        vec![host],
        "idle recovery must neither replace nor duplicate the control center"
    );

    first.disconnect_stdin();
    second.disconnect_stdin();
    assert!(first.close().success());
    assert!(second.close().success());
    wait_for_process_exit(host, PROCESS_DEADLINE);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn thin_proxy_stays_within_a_measured_memory_and_thread_ceiling_after_real_tools() {
    let _serial = runtime_guard();
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("sample.txt"), "sample\n").unwrap();
    let event_log = temp.path().join("runtime-events.log");
    let mut session = McpSession::start(server_command(&home, &workspace, &event_log));
    let response = session.call("glob", serde_json::json!({"pattern": "**/*"}));
    assert_eq!(response["result"]["isError"], false);
    let response = session.call(
        "run",
        serde_json::json!({"command": "printf complete", "login_shell": false}),
    );
    assert!(mcp_text(&response).starts_with("complete"));
    std::thread::sleep(Duration::from_millis(100));
    let (private_bytes, threads) = process_metrics(session.child_id());
    eprintln!(
        "thin proxy metrics: {:.2} MiB private, {threads} threads",
        private_bytes as f64 / (1024.0 * 1024.0)
    );
    assert!(
        private_bytes <= 8 * 1024 * 1024,
        "thin proxy used {} MiB",
        private_bytes as f64 / (1024.0 * 1024.0)
    );
    assert!(threads <= 8, "thin proxy retained {threads} threads");

    let _ = session.kill_proxy();
    terminate_process(host_start_pids(&event_log)[0]);
}

fn runtime_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn server_command(home: &Path, cwd: &Path, event_log: &Path) -> Command {
    let temp = home.join("tmp");
    let local = home.join("local-app-data");
    let runtime = home.join("runtime");
    for directory in [home, &temp, &local, &runtime] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .args(["serve", "--enable-shell"])
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", &local)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .env("FASTCTX_TEST_RUNTIME_EVENT_LOG", event_log)
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", "60000")
        .env("FASTCTX_NO_PARENT_WATCH", "1")
        .env_remove("FASTCTX_TOKEN_BUDGET")
        .env_remove("FASTCTX_READ_TOKEN_BUDGET")
        .env_remove("FASTCTX_GREP_TOKEN_BUDGET")
        .env_remove("FASTCTX_GLOB_TOKEN_BUDGET")
        .env_remove("FASTCTX_RUN_TOKEN_BUDGET")
        .env_remove("FASTCTX_JOB_OUTPUT_TOKEN_BUDGET");
    configure_isolated_process_group(&mut command);
    command
}

fn prepend_path(command: &mut Command, directory: &Path) {
    let mut paths = vec![directory.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    command.env("PATH", std::env::join_paths(paths).unwrap());
}

fn write_path_command(directory: &Path, value: &str) {
    let path = directory.join("session-value");
    std::fs::write(&path, format!("#!/usr/bin/env bash\nprintf '{value}\\n'\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn started_job_id(text: &str) -> String {
    text.lines()
        .find_map(|line| {
            line.strip_prefix("(Complete: job ")
                .and_then(|rest| rest.split_once(" started;").map(|(id, _)| id.to_string()))
        })
        .unwrap_or_else(|| panic!("missing job id in {text}"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn wait_for_host_starts(path: &Path, count: usize, timeout: Duration) -> Vec<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let pids = host_start_pids(path);
        if pids.len() >= count {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "only {} host starts observed",
            pids.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn host_start_pids(path: &Path) -> Vec<u32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.strip_prefix("START ")?.parse().ok())
        .collect()
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "file did not appear: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_text(path: &Path, expected: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if std::fs::read_to_string(path)
            .ok()
            .is_some_and(|value| value.trim() == expected)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "file did not contain {expected:?}: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_path_absence(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "path was not reaped: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_process_exit(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while process_is_alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn configure_isolated_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: pre_exec performs only the async-signal-safe setsid syscall.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(windows)]
fn configure_isolated_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(windows)]
fn kill_proxy_tree(pid: u32) {
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "taskkill failed for proxy {pid}: {status}"
    );
}

#[cfg(unix)]
fn kill_proxy_tree(pid: u32) {
    let group = i32::try_from(pid).unwrap();
    let result = unsafe { libc::kill(-group, libc::SIGKILL) };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let alive = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe { CloseHandle(handle) };
    alive
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let pid = i32::try_from(pid).unwrap();
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn terminate_process(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return;
    }
    unsafe {
        TerminateProcess(handle, 1);
        WaitForSingleObject(handle, 5_000);
        CloseHandle(handle);
    }
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    let pid = i32::try_from(pid).unwrap();
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    wait_for_process_exit(pid as u32, PROCESS_DEADLINE);
}

#[cfg(windows)]
fn process_metrics(pid: u32) -> (u64, u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };

    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    assert!(!process.is_null(), "cannot open proxy {pid}");
    let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
    let read = unsafe {
        GetProcessMemoryInfo(
            process,
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    };
    unsafe { CloseHandle(process) };
    assert_ne!(read, 0, "{}", std::io::Error::last_os_error());

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    assert_ne!(snapshot, INVALID_HANDLE_VALUE);
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut threads = 0_u32;
    let mut present = unsafe { Thread32First(snapshot, &mut entry) };
    while present != 0 {
        if entry.th32OwnerProcessID == pid {
            threads += 1;
        }
        present = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    (memory.PrivateUsage as u64, threads)
}

#[cfg(target_os = "linux")]
fn process_metrics(pid: u32) -> (u64, u32) {
    // RSS and PSS charge shared executable pages that Windows PrivateUsage excludes. Keep the
    // cross-platform ceiling on process-private pages. (2026-08-02)
    let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap();
    let private_kib = ["Private_Clean:", "Private_Dirty:"]
        .into_iter()
        .map(|prefix| {
            rollup
                .lines()
                .find_map(|line| {
                    line.strip_prefix(prefix)?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .unwrap_or_else(|| panic!("Linux smaps_rollup must publish {prefix}"))
        })
        .sum::<u64>();
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let threads = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:")?.trim().parse::<u32>().ok())
        .unwrap();
    (private_kib * 1024, threads)
}
