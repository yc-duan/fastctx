mod common;

use common::{McpSession, mcp_text, normalized};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

#[test]
fn enable_shell_adds_exactly_five_tools_to_the_file_server() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), None);
    assert_eq!(
        session.list_tools(),
        [
            "glob",
            "grep",
            "job_kill",
            "job_list",
            "job_output",
            "read",
            "replace",
            "run",
            "run_background",
        ]
    );
    assert!(session.close().success());
}

#[test]
fn foreground_run_preserves_order_normalizes_output_and_marks_long_line_loss() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), None);
    let response = session.call(
        "run",
        serde_json::json!({
            "command": "printf 'one\\n'; printf 'two\\n' >&2; printf '\\033[31mthree\\033[0m\\rprogress\\r'; printf '\\377'; exit 42"
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        mcp_text(&response),
        "one\ntwo\nthree\nprogress\n�\n\n(Complete: exited 42; 5 lines.)"
    );

    let long = session.call(
        "run",
        serde_json::json!({"command": "printf '%0400000d' 0"}),
    );
    let text = mcp_text(&long);
    assert!(text.starts_with(&"0".repeat(2_000)));
    assert!(text.contains("... [line truncated: 400000 chars total]"));
    assert!(text.ends_with(
        "(Partial: exited 0; 1 line shown, but one or more long lines were truncated at 2000 chars. Redirect to a file (command > file 2>&1) and inspect the long line with the read tool's hex view or grep.)"
    ));
    assert_no_shell_artifacts(temp.path());
    assert!(session.close().success());
}

#[test]
fn foreground_budget_uses_head_and_tail_without_writing_a_spill_file() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), Some("300"));
    let response = session.call(
        "run",
        serde_json::json!({
            "command": "for i in {1..200}; do printf 'line-%03d payload payload payload\\n' \"$i\"; done",
            "login_shell": false
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    let text = mcp_text(&response);
    assert!(text.contains("line-001"), "{text}");
    assert!(text.contains("line-200"), "{text}");
    assert!(text.contains("... ["), "{text}");
    assert!(text.contains(" of 200 lines; exited 0."), "{text}");
    assert!(text.ends_with(
        "Re-run with output redirected to a file (command > file 2>&1) and page it with the read tool.)"
    ));
    assert_no_shell_artifacts(temp.path());
    assert!(session.close().success());
}

#[test]
fn foreground_output_over_eight_mib_runs_to_natural_exit_and_reports_true_line_count() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), Some("1000"));
    let response = session.call(
        "run",
        serde_json::json!({
            "command": "printf -v payload '%01000d' 0; for i in {1..9000}; do printf '%s\\n' \"$payload\"; done; exit 23",
            "timeout_ms": 120000,
            "login_shell": false
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    let text = mcp_text(&response);
    assert!(
        text.starts_with("(Note: "),
        "ring loss must be explicit: {text}"
    );
    assert!(text.contains(" of 9000 lines; exited 23."), "{text}");
    assert!(text.contains("0000"), "{text}");
    assert_no_shell_artifacts(temp.path());
    assert!(session.close().success());
}

#[test]
fn foreground_timeout_kills_descendants_and_keeps_captured_output() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("orphan.txt");
    let mut session = shell_session(temp.path(), None);
    let complete = session.call(
        "run",
        serde_json::json!({"command": "true", "login_shell": false}),
    );
    assert_eq!(mcp_text(&complete), "(Complete: exited 0; no output.)");

    // A non-login shell starts deterministically fast (it never sources
    // /etc/profile), so `printf started` reliably runs inside the 500 ms window
    // before the kill. A login shell's startup can exceed 500 ms under heavy
    // concurrent load, killing the command mid-startup and losing the captured
    // line — a test-timing artifact, not a product fault. The tree-kill semantics
    // under test are identical in either mode; the background non-login timeout
    // test covers the login-independent path too.
    let response = session.call(
        "run",
        serde_json::json!({
            "command": format!(
                "(sleep 1; printf orphan > {}) & printf started; sleep 5",
                bash_quote(&marker)
            ),
            "timeout_ms": 500,
            "login_shell": false
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        mcp_text(&response),
        "started\n\n(Partial: timed out after 500 ms and the process tree was killed; 1 line captured. Increase timeout_ms or use run_background.)"
    );
    std::thread::sleep(Duration::from_millis(1_300));
    assert!(
        !marker.exists(),
        "a timed-out descendant survived the tree kill"
    );
    assert!(session.close().success());
}

#[test]
fn unusable_run_budget_refuses_before_the_command_can_write() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("must-not-exist.txt");
    let mut session = shell_session(temp.path(), Some("1"));
    let response = session.call(
        "run",
        serde_json::json!({
            "command": format!("printf touched > {}", bash_quote(&marker))
        }),
    );
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        mcp_text(&response),
        "FASTCTX_RUN_TOKEN_BUDGET=1 is too small to return the required status note. Increase it and retry."
    );
    assert!(!marker.exists());
    let background = session.call("run_background", serde_json::json!({"command": "sleep 10"}));
    let background_id = started_job_id(mcp_text(&background));
    let killed = session.call("job_kill", serde_json::json!({"job_id": background_id}));
    assert_eq!(
        mcp_text(&killed),
        format!("(Complete: job {background_id} killed.)")
    );
    assert!(session.close().success());
}

#[test]
fn job_output_budget_is_independent_and_inherits_the_global_ceiling() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut command = shell_command(temp.path(), None);
    command.env("FASTCTX_JOB_OUTPUT_TOKEN_BUDGET", "1");
    let mut session = McpSession::start(command);
    let started = session.call(
        "run_background",
        serde_json::json!({"command": "printf output; sleep 10"}),
    );
    let job_id = started_job_id(mcp_text(&started));
    let output = session.call(
        "job_output",
        serde_json::json!({"job_id": job_id, "wait_ms": 2_000}),
    );
    assert_eq!(output["result"]["isError"], true);
    assert_eq!(
        mcp_text(&output),
        "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET=1 is too small to return the required status note. Increase it and retry."
    );
    let killed = session.call("job_kill", serde_json::json!({"job_id": job_id}));
    assert_eq!(
        mcp_text(&killed),
        format!("(Complete: job {job_id} killed.)")
    );
    assert!(session.close().success());
}

#[test]
fn background_default_cursor_and_explicit_after_seq_are_lossless_and_idempotent() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), None);

    let started = session.call(
        "run_background",
        serde_json::json!({
            "command": "printf 'one\\n'; sleep 0.25; printf 'two\\n'; exit 7",
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&started));
    let mut delivered = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "job never reached Complete");
        let output = session.call(
            "job_output",
            serde_json::json!({"job_id": job_id, "wait_ms": 2_000}),
        );
        let text = mcp_text(&output);
        delivered.extend(job_body_lines(text));
        if text.lines().last().unwrap().starts_with("(Complete:") {
            assert_eq!(
                text.lines().last().unwrap(),
                format!("(Complete: job {job_id} exited 7; 2 lines total.)")
            );
            break;
        }
    }
    assert_eq!(delivered, ["one", "two"]);
    let drained = session.call(
        "job_output",
        serde_json::json!({"job_id": job_id, "wait_ms": 0}),
    );
    assert_eq!(
        mcp_text(&drained),
        format!("(Complete: job {job_id} exited 7; 2 lines total.)")
    );
    let already = session.call("job_kill", serde_json::json!({"job_id": job_id}));
    assert_eq!(
        mcp_text(&already),
        format!("(Complete: job {job_id} had already exited with code 7.)")
    );

    let replay_started = session.call(
        "run_background",
        serde_json::json!({
            "command": "printf 'alpha\\nbeta\\n'; sleep 10",
            "login_shell": false
        }),
    );
    let replay_id = started_job_id(mcp_text(&replay_started));
    // Poll from a fixed anchor until both lines are delivered. after_seq=0 always
    // re-anchors from the start, so the job's login-shell startup latency (which
    // can exceed a single wait_ms window under heavy concurrent load) cannot make
    // this flaky. printf writes both lines at once, so it is never caught at one.
    let deadline = Instant::now() + Duration::from_secs(15);
    let first_text = loop {
        assert!(
            Instant::now() < deadline,
            "replay job never delivered both lines"
        );
        let first = session.call(
            "job_output",
            serde_json::json!({"job_id": replay_id, "wait_ms": 2_000, "after_seq": 0}),
        );
        let text = mcp_text(&first).to_string();
        if text.contains("after_seq=2") {
            break text;
        }
    };
    // A retry from the same anchor is idempotent across server instances: seq is durable,
    // not a cursor assigned by the server process.
    assert!(session.close().success());
    let mut second = shell_session(temp.path(), None);
    let retry = second.call(
        "job_output",
        serde_json::json!({"job_id": replay_id, "wait_ms": 0, "after_seq": 0}),
    );
    assert_eq!(mcp_text(&retry), first_text);
    let killed = second.call("job_kill", serde_json::json!({"job_id": replay_id}));
    assert_eq!(
        mcp_text(&killed),
        format!("(Complete: job {replay_id} killed.)")
    );
    assert_no_shell_artifacts(temp.path());
    assert!(second.close().success());
}

#[test]
fn background_has_no_timeout_and_non_login_shell_has_a_complete_fast_path() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join(".bash_profile"),
        b"export FASTCTX_PROFILE_VALUE=loaded\n",
    )
    .unwrap();
    let mut command = shell_command(temp.path(), None);
    command.env("HOME", temp.path());
    let mut session = McpSession::start(command);

    let login = session.call(
        "run",
        serde_json::json!({"command": "printf '%s' \"${FASTCTX_PROFILE_VALUE:-missing}\""}),
    );
    assert_eq!(mcp_text(&login), "loaded\n\n(Complete: exited 0; 1 line.)");
    let clean = session.call(
        "run",
        serde_json::json!({
            "command": "printf '%s' \"${FASTCTX_PROFILE_VALUE:-missing}\"",
            "login_shell": false
        }),
    );
    assert_eq!(mcp_text(&clean), "missing\n\n(Complete: exited 0; 1 line.)");

    let rejected = session.call(
        "run_background",
        serde_json::json!({
            "command": "true",
            "timeout_ms": 500
        }),
    );
    let serialized = serde_json::to_string(&rejected).unwrap();
    assert!(
        serialized.contains("timeout_ms") && serialized.contains("unknown field"),
        "{serialized}"
    );
    assert!(session.close().success());
}

#[test]
fn global_background_limit_and_job_ids_survive_across_server_instances() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    write_job_settings(temp.path(), 2, 1_024);
    let mut session = shell_session(temp.path(), None);
    let mut ids = Vec::new();
    for _ in 0..2 {
        let response = session.call(
            "run_background",
            serde_json::json!({"command": "sleep 10", "login_shell": false}),
        );
        ids.push(started_job_id(mcp_text(&response)));
    }
    assert!(ids.iter().all(|id| valid_job_id(id)));
    let over = session.call(
        "run_background",
        serde_json::json!({"command": "printf should-not-start"}),
    );
    assert_eq!(over["result"]["isError"], true);
    assert_eq!(
        mcp_text(&over),
        "Too many running jobs: the limit is 2 across all FastCtx sessions for the current user. Kill or wait out an existing job first."
    );
    assert!(session.close().success());

    let mut second = shell_session(temp.path(), None);
    let listed = second.call("job_list", serde_json::json!({}));
    let list_text = mcp_text(&listed);
    for id in &ids {
        assert!(
            list_text.contains(&format!("{id}  running; started ")),
            "{list_text}"
        );
        let output = second.call(
            "job_output",
            serde_json::json!({"job_id": id, "wait_ms": 0}),
        );
        assert!(
            mcp_text(&output).ends_with(&format!(
                "(Partial: job {id} is running; no new output within 0 ms. Call job_output again.)"
            )),
            "{}",
            mcp_text(&output)
        );
        let killed = second.call("job_kill", serde_json::json!({"job_id": id}));
        assert_eq!(mcp_text(&killed), format!("(Complete: job {id} killed.)"));
    }
    assert!(second.close().success());
}

#[test]
fn job_list_defaults_to_running_uses_the_saved_page_size_and_requires_explicit_history() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    write_job_settings_with_list_limit(temp.path(), 4, 1_024, 1);
    let mut session = shell_session(temp.path(), None);

    let finished = started_job_id(mcp_text(&session.call(
        "run_background",
        serde_json::json!({"command": "printf finished", "login_shell": false}),
    )));
    let _ = wait_for_complete_from(&mut session, &finished, Some(0));
    let running = (0..2)
        .map(|_| {
            started_job_id(mcp_text(&session.call(
                "run_background",
                serde_json::json!({"command": "sleep 10", "login_shell": false}),
            )))
        })
        .collect::<Vec<_>>();

    let default_page = mcp_text(&session.call("job_list", serde_json::json!({}))).to_string();
    assert!(default_page.contains(&running[1]), "{default_page}");
    assert!(!default_page.contains(&running[0]), "{default_page}");
    assert!(!default_page.contains(&finished), "{default_page}");
    assert!(
        default_page.ends_with("Call job_list again with status=\"running\", limit=1, offset=1.)"),
        "{default_page}"
    );

    let finished_page =
        mcp_text(&session.call("job_list", serde_json::json!({"status": "finished"}))).to_string();
    assert!(finished_page.contains(&finished), "{finished_page}");
    assert!(!finished_page.contains(&running[0]), "{finished_page}");

    let all = mcp_text(&session.call(
        "job_list",
        serde_json::json!({"status": "all", "limit": 100}),
    ))
    .to_string();
    assert!(all.contains(&finished), "{all}");
    assert!(running.iter().all(|id| all.contains(id)), "{all}");

    for id in running {
        let killed = session.call("job_kill", serde_json::json!({"job_id": id}));
        assert_eq!(killed["result"]["isError"], false, "{killed}");
    }
    assert!(session.close().success());
}

#[test]
fn concurrent_servers_cannot_oversubscribe_the_machine_job_limit() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    write_job_settings(temp.path(), 1, 1_024);
    let first = shell_session(temp.path(), None);
    let second = shell_session(temp.path(), None);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

    let first_barrier = barrier.clone();
    let first_start = std::thread::spawn(move || {
        let mut session = first;
        first_barrier.wait();
        let response = session.call(
            "run_background",
            serde_json::json!({"command": "sleep 10", "login_shell": false}),
        );
        (session, response)
    });
    let second_barrier = barrier.clone();
    let second_start = std::thread::spawn(move || {
        let mut session = second;
        second_barrier.wait();
        let response = session.call(
            "run_background",
            serde_json::json!({"command": "sleep 10", "login_shell": false}),
        );
        (session, response)
    });
    barrier.wait();

    let (first, first_response) = first_start.join().unwrap();
    let (second, second_response) = second_start.join().unwrap();
    let responses = [&first_response, &second_response];
    let started = responses
        .iter()
        .filter(|response| response["result"]["isError"] == false)
        .map(|response| started_job_id(mcp_text(response)))
        .collect::<Vec<_>>();
    let rejected = responses
        .iter()
        .filter(|response| response["result"]["isError"] == true)
        .map(|response| mcp_text(response))
        .collect::<Vec<_>>();
    assert_eq!(started.len(), 1, "{responses:?}");
    assert_eq!(
        rejected,
        [
            "Too many running jobs: the limit is 1 across all FastCtx sessions for the current user. Kill or wait out an existing job first."
        ]
    );
    assert!(first.close().success());
    assert!(second.close().success());

    let mut cleanup = shell_session(temp.path(), None);
    let killed = cleanup.call("job_kill", serde_json::json!({"job_id": &started[0]}));
    assert_eq!(
        mcp_text(&killed),
        format!("(Complete: job {} killed.)", started[0])
    );
    assert!(cleanup.close().success());
}

#[test]
fn detached_job_reaches_complete_after_its_starting_server_exits() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("survived.txt");
    let mut first = shell_session(temp.path(), None);
    let started = first.call(
        "run_background",
        serde_json::json!({
            "command": format!(
                "printf 'one\\n'; sleep 0.4; printf 'two\\n'; printf survived > {}; exit 9",
                bash_quote(&marker)
            ),
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&started));
    assert!(first.close().success());

    let mut second = shell_session(temp.path(), None);
    let final_text = wait_for_complete_from(&mut second, &job_id, Some(0));
    assert_eq!(job_body_lines(&final_text), ["one", "two"]);
    assert_eq!(
        final_text.lines().last().unwrap(),
        format!("(Complete: job {job_id} exited 9; 2 lines total.)")
    );
    assert!(marker.exists());
    assert!(second.close().success());
}

#[test]
fn killing_the_supervisor_reports_interrupted_and_leaves_no_command_descendant() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("must-not-be-written.txt");
    let mut first = shell_session(temp.path(), None);
    let started = first.call(
        "run_background",
        serde_json::json!({
            "command": format!(
                "printf 'started\\n'; (sleep 1; printf orphan > {}) & sleep 30",
                bash_quote(&marker)
            ),
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&started));
    let initial = wait_for_job_text(&mut first, &job_id, "started");
    assert!(initial.contains("after_seq=1"), "{initial}");
    let meta: Value = serde_json::from_slice(
        &std::fs::read(
            temp.path()
                .join(".fastctx")
                .join("jobs")
                .join(&job_id)
                .join("meta.json"),
        )
        .unwrap(),
    )
    .unwrap();
    terminate_process(meta["supervisor"]["pid"].as_u64().unwrap() as u32);
    assert!(first.close().success());

    std::thread::sleep(Duration::from_millis(1_300));
    assert!(
        !marker.exists(),
        "the supervisor left an orphan command descendant"
    );
    let mut second = shell_session(temp.path(), None);
    let interrupted = wait_for_complete_from(&mut second, &job_id, Some(0));
    assert!(interrupted.contains("started"), "{interrupted}");
    assert_eq!(
        interrupted.lines().last().unwrap(),
        format!(
            "(Complete: job {job_id} was interrupted: its process ended without an exit record (machine restart or external kill); 1 line preserved.)"
        )
    );
    let already = second.call("job_kill", serde_json::json!({"job_id": job_id}));
    assert_eq!(
        mcp_text(&already),
        format!("(Complete: job {job_id} had already been interrupted.)")
    );
    assert!(second.close().success());
}

#[test]
fn capture_failure_keeps_the_command_running_and_falls_back_to_the_exit_record() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let continued = temp.path().join("continued.txt");
    let mut session = shell_session(temp.path(), None);
    let started = session.call(
        "run_background",
        serde_json::json!({
            "command": format!(
                "sleep 0.2; printf 'output\\n'; sleep 0.5; printf continued > {}; sleep 1; exit 17",
                bash_quote(&continued)
            ),
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&started));
    let jobs = temp.path().join(".fastctx").join("jobs");
    let original = jobs.join(&job_id);
    let displaced = jobs.join(format!("{job_id}.displaced"));
    std::fs::rename(&original, &displaced).unwrap();
    wait_until(Duration::from_secs(5), || continued.exists());
    std::fs::rename(&displaced, &original).unwrap();

    let final_text = wait_for_complete_from(&mut session, &job_id, Some(0));
    assert!(continued.exists());
    assert!(
        final_text.contains("(Note: output capture failed after seq 0:"),
        "{final_text}"
    );
    assert!(
        final_text.contains("This does not kill the process; its exit status remains available"),
        "{final_text}"
    );
    assert!(
        final_text.ends_with(&format!(
            "(Complete: job {job_id} exited 17; 0 lines total, but some output was dropped or truncated — redirect the command to a file (command > file 2>&1) for the full log.)"
        )),
        "{final_text}"
    );
    assert!(session.close().success());
}

#[test]
fn shell_error_catalog_uses_fastctx_names_and_rejects_invalid_inputs() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut command = shell_command(temp.path(), None);
    command.env("FASTCTX_BASH", "relative/bash");
    let mut session = McpSession::start(command);
    let invalid_bash = session.call("run", serde_json::json!({"command": "printf nope"}));
    assert_eq!(invalid_bash["result"]["isError"], true);
    assert_eq!(
        mcp_text(&invalid_bash),
        "Invalid FASTCTX_BASH value \"relative/bash\": not a working bash (the path is not absolute). Fix or unset it."
    );
    assert!(session.close().success());

    let mut old_name = shell_command(temp.path(), None);
    old_name.env("FASTSHELL_BASH", "relative/bash");
    let mut old_name_session = McpSession::start(old_name);
    let ignored = old_name_session.call("run", serde_json::json!({"command": "true"}));
    assert_eq!(ignored["result"]["isError"], false);
    assert_eq!(mcp_text(&ignored), "(Complete: exited 0; no output.)");
    assert!(old_name_session.close().success());

    let mut session = shell_session(temp.path(), None);
    let cases: [(&str, Value, &str); 11] = [
        (
            "run",
            serde_json::json!({"command": ""}),
            "Invalid command: it must be a non-empty string.",
        ),
        (
            "run",
            serde_json::json!({"command": "true", "cwd": "relative"}),
            "The cwd parameter must be an absolute path.",
        ),
        (
            "run",
            serde_json::json!({"command": "true", "timeout_ms": 0}),
            "Invalid timeout_ms value: 0. Expected an integer from 1 to 240000.",
        ),
        (
            "run",
            serde_json::json!({"command": "true", "timeout_ms": 240001}),
            "Invalid timeout_ms value: 240001. Expected an integer from 1 to 240000.",
        ),
        (
            "run_background",
            serde_json::json!({"command": "  "}),
            "Invalid command: it must be a non-empty string.",
        ),
        (
            "job_output",
            serde_json::json!({"job_id": "missing", "wait_ms": 120001}),
            "Invalid wait_ms value: 120001. Expected an integer from 0 to 120000.",
        ),
        (
            "job_output",
            serde_json::json!({"job_id": "missing", "wait_ms": 0}),
            "No such job: \"missing\". It may never have existed, or its finished record was evicted by the job storage limit. List known jobs with job_list.",
        ),
        (
            "job_kill",
            serde_json::json!({"job_id": "missing"}),
            "No such job: \"missing\". It may never have existed, or its finished record was evicted by the job storage limit. List known jobs with job_list.",
        ),
        (
            "job_list",
            serde_json::json!({"offset": -1}),
            "Invalid offset value: -1. Expected a non-negative integer.",
        ),
        (
            "job_list",
            serde_json::json!({"limit": 0}),
            "Invalid limit value: 0. Expected an integer from 1 to 100.",
        ),
        (
            "job_list",
            serde_json::json!({"limit": 101}),
            "Invalid limit value: 101. Expected an integer from 1 to 100.",
        ),
    ];
    for (tool, arguments, expected) in cases {
        let response = session.call(tool, arguments);
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(mcp_text(&response), expected);
    }
    assert!(session.close().success());

    let missing = temp.path().join("missing-cwd");
    let ordinary_file = temp.path().join("not-a-directory.txt");
    std::fs::write(&ordinary_file, b"file").unwrap();
    let mut session = shell_session(temp.path(), None);
    for (cwd, expected) in [
        (
            normalized(&missing),
            format!("Working directory does not exist: {}", normalized(&missing)),
        ),
        (
            normalized(&ordinary_file),
            format!(
                "Working directory is not a directory: {}",
                normalized(&ordinary_file)
            ),
        ),
    ] {
        let response = session.call("run", serde_json::json!({"command": "true", "cwd": cwd}));
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(mcp_text(&response), expected);
    }
    assert!(session.close().success());

    let mut invalid_budget = shell_session(temp.path(), Some("not-a-number"));
    let response = invalid_budget.call("run", serde_json::json!({"command": "true"}));
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        mcp_text(&response),
        "Invalid FASTCTX_RUN_TOKEN_BUDGET value \"not-a-number\": expected a positive integer."
    );
    assert!(invalid_budget.close().success());
}

/// Locks the Windows PATH augmentation: a clean (non-login) shell must still find
/// the Unix toolset even when the host that launched the server has no Git
/// directory on PATH (e.g. Codex started from PowerShell). Removing the
/// augmentation makes every external command fail with 127 and this test red.
#[cfg(windows)]
#[test]
fn non_login_shell_finds_the_unix_toolset_without_git_on_the_host_path() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut command = shell_command(temp.path(), None);
    // A host PATH stripped down to the Windows system directories — no Git usr/bin.
    // bash is still discovered via the standard install path, not via this PATH.
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    command.env("PATH", format!("{system_root}\\System32;{system_root}"));
    let mut session = McpSession::start(command);
    let response = session.call(
        "run",
        serde_json::json!({
            "command": "sleep 0.01 && sed --version >/dev/null && printf ok",
            "login_shell": false
        }),
    );
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(mcp_text(&response), "ok\n\n(Complete: exited 0; 1 line.)");
    assert!(session.close().success());
}

fn shell_contract_guard() -> MutexGuard<'static, ()> {
    // These cases launch detached process trees. Keeping unrelated scenarios
    // isolated prevents OS process and pipe pressure from becoming test input.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn shell_session(root: &Path, run_budget: Option<&str>) -> McpSession {
    McpSession::start(shell_command(root, run_budget))
}

fn shell_command(root: &Path, run_budget: Option<&str>) -> Command {
    let temp = root.join("tmp");
    std::fs::create_dir_all(&temp).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .args(["serve", "--enable-shell"])
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp);
    if let Some(budget) = run_budget {
        command.env("FASTCTX_RUN_TOKEN_BUDGET", budget);
    }
    command
}

fn bash_quote(path: &Path) -> String {
    format!("'{}'", normalized(path).replace('\'', "'\\''"))
}

fn started_job_id(text: &str) -> String {
    let id = text
        .strip_prefix("(Complete: job ")
        .and_then(|value| value.strip_suffix(" started.)"))
        .unwrap_or_else(|| {
            panic!("run_background must return the frozen start terminal; got {text:?}")
        });
    assert!(valid_job_id(id), "{id}");
    id.to_string()
}

fn valid_job_id(id: &str) -> bool {
    id.len() == 8
        && id.starts_with("j-")
        && id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

fn job_body_lines(text: &str) -> Vec<String> {
    let Some((body, _)) = text.rsplit_once("\n\n(") else {
        return Vec::new();
    };
    body.lines()
        .filter(|line| !line.starts_with("(Note:"))
        .map(ToOwned::to_owned)
        .collect()
}

fn write_job_settings(root: &Path, max_running_jobs: u64, job_storage_limit_mib: u64) {
    write_job_settings_with_list_limit(root, max_running_jobs, job_storage_limit_mib, 20);
}

fn write_job_settings_with_list_limit(
    root: &Path,
    max_running_jobs: u64,
    job_storage_limit_mib: u64,
    job_list_limit: u64,
) {
    let directory = root.join(".fastctx");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        format!(
            "schema_version = 1\n\n[fastshell]\nenabled = true\njob_storage_limit_mib = {job_storage_limit_mib}\nmax_running_jobs = {max_running_jobs}\njob_list_limit = {job_list_limit}\n"
        ),
    )
    .unwrap();
}

fn wait_for_complete_from(
    session: &mut McpSession,
    job_id: &str,
    after_seq: Option<u64>,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(Instant::now() < deadline, "job {job_id} never completed");
        let mut arguments = serde_json::json!({
            "job_id": job_id,
            "wait_ms": 2_000
        });
        if let Some(after_seq) = after_seq {
            arguments["after_seq"] = after_seq.into();
        }
        let output = session.call("job_output", arguments);
        let text = mcp_text(&output).to_string();
        if text
            .lines()
            .last()
            .is_some_and(|line| line.starts_with("(Complete:"))
        {
            return text;
        }
    }
}

fn wait_for_job_text(session: &mut McpSession, job_id: &str, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            Instant::now() < deadline,
            "job {job_id} never produced {needle:?}"
        );
        let output = session.call(
            "job_output",
            serde_json::json!({"job_id": job_id, "wait_ms": 2_000, "after_seq": 0}),
        );
        let text = mcp_text(&output).to_string();
        if text.contains(needle) {
            return text;
        }
    }
}

fn wait_until(mut timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let step = Duration::from_millis(20);
    while !predicate() {
        assert!(!timeout.is_zero(), "condition did not become true in time");
        let delay = timeout.min(step);
        std::thread::sleep(delay);
        timeout = timeout.saturating_sub(delay);
    }
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    // SAFETY: SIGKILL is sent to the exact supervisor PID read from its immutable
    // metadata; the test immediately verifies the resulting interrupted state.
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    assert_eq!(
        result,
        0,
        "failed to terminate supervisor {pid}: {}",
        std::io::Error::last_os_error()
    );
}

#[cfg(windows)]
fn terminate_process(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: the handle is opened only with PROCESS_TERMINATE for the supervisor
    // PID stored in immutable job metadata and is closed on every successful open.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(
            !process.is_null(),
            "failed to open supervisor {pid}: {}",
            std::io::Error::last_os_error()
        );
        let terminated = TerminateProcess(process, 1);
        let error = std::io::Error::last_os_error();
        let closed = CloseHandle(process);
        assert_ne!(
            terminated, 0,
            "failed to terminate supervisor {pid}: {error}"
        );
        assert_ne!(
            closed,
            0,
            "failed to close supervisor handle {pid}: {}",
            std::io::Error::last_os_error()
        );
    }
}

fn assert_no_shell_artifacts(root: &Path) {
    let shell_dir = root.join("fastctx-shell");
    assert!(
        !shell_dir.exists(),
        "shell created {}",
        normalized(&shell_dir)
    );
    let logs = std::fs::read_dir(root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
        .map(|entry| normalized(&entry.path()))
        .collect::<Vec<_>>();
    assert!(logs.is_empty(), "shell created log artifacts: {logs:?}");
}
