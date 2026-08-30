mod common;

use common::{McpSession, mcp_text, normalized};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

#[test]
fn background_status_tracks_only_known_jobs_and_delivers_each_terminal_edge_once() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let probe = temp.path().join("probe.txt");
    std::fs::write(&probe, "probe\n").unwrap();
    let read_arguments = serde_json::json!({"file_path": normalized(&probe)});
    let mut session = shell_session(temp.path(), None);

    let running_start = session.call(
        "run_background",
        serde_json::json!({"command": "sleep 30", "login_shell": false}),
    );
    let running = started_job_id(mcp_text(&running_start));
    assert!(background_line(mcp_text(&running_start)).is_none());

    let read = session.call("inspect_local_file", read_arguments.clone());
    let read_text = mcp_text(&read);
    let read_lines = read_text.lines().collect::<Vec<_>>();
    assert!(read_lines[0].starts_with("=== "), "{read_text}");
    assert!(
        read_lines[1].starts_with(&format!("=== jobs: {running} running ")),
        "{read_text}"
    );
    assert!(read_lines.contains(&"1\tprobe"), "{read_text}");

    let missing = session.call(
        "inspect_local_file",
        serde_json::json!({"file_path": normalized(&temp.path().join("missing.txt"))}),
    );
    assert_eq!(missing["result"]["isError"], true);
    assert!(background_line(mcp_text(&missing)).is_none());

    let mut other = shell_session(temp.path(), None);
    let isolated = other.call("inspect_local_file", read_arguments.clone());
    assert!(background_line(mcp_text(&isolated)).is_none());
    assert!(other.close().success());

    let finished_start = session.call(
        "run_background",
        serde_json::json!({"command": "exit 7", "login_shell": false}),
    );
    let finished = started_job_id(mcp_text(&finished_start));
    let start_status = background_line(mcp_text(&finished_start)).unwrap();
    assert!(start_status.contains(&running), "{start_status}");
    assert!(!start_status.contains(&finished), "{start_status}");

    let exit_record = temp
        .path()
        .join(".fastctx")
        .join("jobs")
        .join(&finished)
        .join("exit.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !exit_record.exists() {
        assert!(Instant::now() < deadline, "job {finished} did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }

    let both = session.call("inspect_local_file", read_arguments.clone());
    let both_status = background_line(mcp_text(&both)).unwrap();
    assert!(both_status.contains(&format!("{running} running ")));
    assert!(both_status.contains(&format!("{finished} exited 7")));

    let listed = session.call(
        "job_list",
        serde_json::json!({"status": "all", "limit": 100}),
    );
    let list_status = background_line(mcp_text(&listed)).unwrap();
    assert!(list_status.contains(&running));
    assert!(!list_status.contains(&finished));
    let after_list = session.call("inspect_local_file", read_arguments.clone());
    let after_list_status = background_line(mcp_text(&after_list)).unwrap();
    assert!(after_list_status.contains(&running));
    assert!(!after_list_status.contains(&finished));

    let consumed = session.call(
        "job_output",
        serde_json::json!({"job_id": &finished, "wait_ms": 0}),
    );
    let consumed_status = background_line(mcp_text(&consumed)).unwrap();
    assert!(consumed_status.contains(&running));
    assert!(!consumed_status.contains(&finished));
    let after_output = session.call("inspect_local_file", read_arguments.clone());
    let after_output_status = background_line(mcp_text(&after_output)).unwrap();
    assert!(after_output_status.contains(&running));
    assert!(!after_output_status.contains(&finished));

    let killed = session.call("job_kill", serde_json::json!({"job_id": &running}));
    assert!(background_line(mcp_text(&killed)).is_none());
    let empty = session.call("inspect_local_file", read_arguments);
    assert!(background_line(mcp_text(&empty)).is_none());
    assert!(session.close().success());
}

#[test]
fn foreground_output_over_eight_mib_runs_to_natural_exit_and_reports_true_line_count() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), Some("1000"));
    // This fixture deliberately pushes more than 8 MiB through debug output fitting;
    // it is a correctness contract, not a ten-second latency contract.
    let response = session.call_with_timeout(
        "run",
        serde_json::json!({
            "command": "printf -v payload '%01000d' 0; for i in {1..9000}; do printf '%s\\n' \"$payload\"; done; exit 23",
            "timeout_ms": 120000,
            "login_shell": false
        }),
        Duration::from_secs(60),
    );
    assert_eq!(response["result"]["isError"], false);
    let text = mcp_text(&response);
    assert!(
        text.starts_with("=== run (lines "),
        "ring loss must be explicit: {text}"
    );
    assert!(text.contains(" of 9000; exited 23"), "{text}");
    assert!(text.contains("dropped from the in-memory buffer"), "{text}");
    assert!(text.contains("0000"), "{text}");
    assert_no_shell_artifacts(temp.path());
    assert!(session.close().success());
}

#[test]
fn foreground_timeout_kills_descendants_and_keeps_captured_output() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let spawned = temp.path().join("spawned.txt");
    let marker = temp.path().join("orphan.txt");
    let mut session = shell_session(temp.path(), None);
    let complete = session.call(
        "run",
        serde_json::json!({"command": "true", "login_shell": false}),
    );
    assert_eq!(mcp_text(&complete), "=== run (0 lines; exited 0) ===");

    // Two independent deadlines have to hold: the shell must reach `printf
    // started` before the kill, and the descendant must still be sleeping when
    // the kill lands. A login shell is avoided because sourcing /etc/profile can
    // outlast either window, and the timeout is 2000 ms because a cold Git Bash
    // on a loaded CI runner has needed more than 500 ms just to start, which lost
    // the captured line and failed this test for a reason the product had no part
    // in. The descendant writes `spawned.txt` immediately, so a shell too slow to
    // fork it cannot pass the tree-kill assertion vacuously. Every gap here is one
    // second wide; do not shrink them back. (2026-07-25)
    let response = session.call(
        "run",
        serde_json::json!({
            "command": format!(
                "(printf spawned > {}; sleep 3; printf orphan > {}) & printf started; sleep 10",
                bash_quote(&spawned),
                bash_quote(&marker)
            ),
            "timeout_ms": 2_000,
            "login_shell": false
        }),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        mcp_text(&response),
        "=== run (1 line; timed out after 2000 ms, process tree killed) ===\nstarted"
    );
    assert!(
        spawned.exists(),
        "the descendant never started, so the tree kill would prove nothing"
    );
    std::thread::sleep(Duration::from_millis(2_500));
    assert!(
        !marker.exists(),
        "a timed-out descendant survived the tree kill"
    );
    assert!(session.close().success());
}

/// A command longer than one Windows command line must still run: over the script-spawn
/// threshold the text is handed to bash as a temp script, so no host length cap applies.
/// Passed as a bare `bash -c` argument it fails CreateProcessW with os error 206
/// (2026-08-08).

#[test]
fn background_log_over_eight_mib_keeps_a_directly_readable_omission_coordinate() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut command = shell_command(temp.path(), None);
    // Keep enough room for the mandatory status/path notes while still forcing a
    // tiny response window over the 9 MiB fixture.
    command.env("FASTCTX_JOB_OUTPUT_TOKEN_BUDGET", "1000");
    let mut session = McpSession::start(command);
    let started = session.call(
        "run_background",
        serde_json::json!({
            "command": "printf -v payload '%01000d' 0; yes \"$payload\" | head -n 9000",
            "login_shell": false
        }),
    );
    let start_text = mcp_text(&started);
    let job_id = started_job_id(start_text);
    let log_path = started_job_log(start_text);
    let output =
        wait_for_terminal_from_within(&mut session, &job_id, Some(0), Duration::from_secs(60));

    assert!(output.contains(&format!("log at {log_path}")), "{output}");
    let omitted = omitted_start(&output);
    assert!(
        output.contains(&format!("complete log at {log_path}")),
        "{output}"
    );
    let recovered = session.call(
        "inspect_local_file",
        serde_json::json!({
            "file_path": &log_path,
            "offset": omitted,
            "limit": 1
        }),
    );
    assert_eq!(recovered["result"]["isError"], false, "{recovered}");
    assert!(
        mcp_text(&recovered).contains(&format!("\n{omitted}\t{}", "0".repeat(1_000))),
        "{}",
        mcp_text(&recovered)
    );
    assert!(std::fs::metadata(&log_path).unwrap().len() > 8 * 1024 * 1024);
    let job_dir = Path::new(&log_path).parent().unwrap();
    assert!(job_dir.join("output.idx").is_file());
    assert!(
        std::fs::read_dir(job_dir)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with("segment-"))
    );
    let drained = session.call(
        "job_output",
        serde_json::json!({"job_id": job_id, "wait_ms": 0}),
    );
    assert!(job_body_lines(mcp_text(&drained)).is_empty());
    assert!(mcp_text(&drained).contains("0 new lines"));
    assert!(mcp_text(&drained).contains("log at"));
    assert!(session.close().success());
}

#[test]
fn job_output_waits_through_intermediate_output_until_the_job_ends() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let release_output = temp.path().join("release-output");
    let release_exit = temp.path().join("release-exit");
    let command = format!(
        "printf 'first\\n'; while [ ! -f {} ]; do sleep 0.02; done; printf 'second\\n'; while [ ! -f {} ]; do sleep 0.02; done; exit 9",
        bash_quote(&release_output),
        bash_quote(&release_exit)
    );
    let mut session = shell_session(temp.path(), None);
    let started = session.call(
        "run_background",
        serde_json::json!({"command": command, "login_shell": false}),
    );
    let job_id = started_job_id(mcp_text(&started));

    let output_path = release_output.clone();
    let exit_path = release_exit.clone();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(output_path, b"go").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(exit_path, b"go").unwrap();
    });
    let waiting_started = Instant::now();
    let exited = session.call(
        "job_output",
        serde_json::json!({
            "job_id": job_id,
            "wait_ms": 2_000,
            "after_seq": 0
        }),
    );
    releaser.join().unwrap();
    assert!(
        waiting_started.elapsed() >= Duration::from_millis(450),
        "intermediate output ended a query that should wait for terminal state"
    );
    let text = mcp_text(&exited);
    assert_eq!(job_body_lines(text), ["first", "second"]);
    assert!(
        text.starts_with(&format!(
            "=== job {job_id} exited 9 (lines 1-2 of 2; log at "
        )),
        "{text}"
    );
    assert!(session.close().success());
}

#[test]
fn job_output_wait_window_delivers_accumulated_output_without_returning_on_each_line() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), None);
    // The window has to outlast a cold Git Bash start. A CI runner can spend
    // several hundred milliseconds before the command produces its first line,
    // which a 400ms window swallowed whole: only "first" had arrived when the
    // wait expired. Three seconds keeps the accumulation claim intact - the
    // elapsed assertion below still proves the call waited out the full window
    // instead of returning on the first line - while leaving room for a slow
    // start. The trailing sleep outlasts the window so the job stays running.
    // (2026-07-25)
    let started = session.call(
        "run_background",
        serde_json::json!({
            "command": "printf 'first\\n'; sleep 0.05; printf 'second\\n'; sleep 30",
            "login_shell": false
        }),
    );
    let job_id = started_job_id(mcp_text(&started));
    let waiting_started = Instant::now();
    let output = session.call(
        "job_output",
        serde_json::json!({
            "job_id": job_id,
            "wait_ms": 3_000,
            "after_seq": 0
        }),
    );
    assert!(waiting_started.elapsed() >= Duration::from_millis(2_900));
    assert_eq!(job_body_lines(mcp_text(&output)), ["first", "second"]);
    assert!(
        mcp_text(&output).starts_with(&format!(
            "=== job {job_id} running (lines 1-2 of 2; log at "
        )),
        "{}",
        mcp_text(&output)
    );
    let killed = session.call("job_kill", serde_json::json!({"job_id": job_id}));
    assert_eq!(mcp_text(&killed), format!("=== job {job_id} (killed) ==="));
    assert!(session.close().success());
}

/// A stop requested through job_kill must never display as a plain exit code:
/// Windows TerminateProcess hardcodes exit code 1, which reads as a real failure.
/// A fresh-host field test hit exactly this misreport (2026-08-08).
#[test]
fn killed_jobs_report_killed_not_a_synthetic_exit_code() {
    let _serial = shell_contract_guard();
    let temp = tempfile::tempdir().unwrap();
    let mut session = shell_session(temp.path(), None);
    let started = session.call(
        "run_background",
        serde_json::json!({"command": "sleep 30", "login_shell": false}),
    );
    let job_id = started_job_id(mcp_text(&started));
    let killed = session.call("job_kill", serde_json::json!({"job_id": job_id}));
    assert_eq!(mcp_text(&killed), format!("=== job {job_id} (killed) ==="));
    let output = session.call(
        "job_output",
        serde_json::json!({"job_id": job_id, "wait_ms": 0}),
    );
    let text = mcp_text(&output);
    assert!(
        text.starts_with(&format!("=== job {job_id} killed (")),
        "{text}"
    );
    let listed = session.call("job_list", serde_json::json!({"status": "finished"}));
    let listed_text = mcp_text(&listed);
    assert!(
        listed_text.contains(&format!("{job_id}  killed; started ")),
        "{listed_text}"
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
            mcp_text(&output).starts_with(&format!("=== job {id} running (0 new lines; ")),
            "{}",
            mcp_text(&output)
        );
        assert!(mcp_text(&output).contains("no new output within 0 ms"));
        let killed = second.call("job_kill", serde_json::json!({"job_id": id}));
        assert_eq!(mcp_text(&killed), format!("=== job {id} (killed) ==="));
    }
    assert!(second.close().success());
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
        format!("=== job {} (killed) ===", started[0])
    );
    assert!(cleanup.close().success());
}

#[test]
fn detached_job_reaches_terminal_state_after_its_starting_server_exits() {
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
    let final_text = wait_for_terminal_from(&mut second, &job_id, Some(0));
    assert_eq!(job_body_lines(&final_text), ["one", "two"]);
    assert!(
        final_text.starts_with(&format!(
            "=== job {job_id} exited 9 (lines 1-2 of 2; log at "
        )),
        "{final_text}"
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
    assert!(initial.contains("lines 1 of 1"), "{initial}");
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
    let interrupted = wait_for_terminal_from(&mut second, &job_id, Some(0));
    assert!(interrupted.contains("started"), "{interrupted}");
    assert!(
        interrupted.starts_with(&format!(
            "=== job {job_id} interrupted (lines 1 of 1; log at "
        )),
        "{interrupted}"
    );
    assert!(
        interrupted.contains("process ended without an exit record"),
        "{interrupted}"
    );
    let already = second.call("job_kill", serde_json::json!({"job_id": job_id}));
    assert_eq!(
        mcp_text(&already),
        format!("=== job {job_id} (already interrupted) ===")
    );
    assert!(second.close().success());
}

// Provoking a capture failure means making a write fail on a handle the supervisor
// already holds, because the log is opened before `run_background` answers. Windows
// mandatory locking does that from the outside; POSIX offers no equivalent, since
// renaming or unlinking the job directory is invisible to an open descriptor, which
// keeps writing to the same inode. The note itself, and its fallback to the exit
// record, are locked on every platform by the `format_snapshot` unit tests in
// `src/shell/jobs/mod.rs` (2026-07-24).
#[cfg(windows)]
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
    let capture_block = lock_output_log(&original.join("output.log"));
    wait_until(Duration::from_secs(5), || continued.exists());
    drop(capture_block);

    let final_text = wait_for_terminal_from(&mut session, &job_id, Some(0));
    assert!(continued.exists());
    assert!(
        final_text.contains("output capture failed after stored line 0:"),
        "{final_text}"
    );
    assert!(
        final_text.contains("the process was not killed"),
        "{final_text}"
    );
    assert!(
        final_text.contains("the log at ") && final_text.contains(" stops there"),
        "{final_text}"
    );
    assert!(
        final_text.starts_with(&format!("=== job {job_id} exited 17 (0 new lines; log at ")),
        "{final_text}"
    );
    assert!(session.close().success());
}

/// Serializes shell contracts whose detached process trees would otherwise make
/// OS process and pipe pressure part of unrelated scenarios.
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
        .args([
            "serve",
            "--tools",
            "inspect_local_file,grep,glob,replace,run,run_background,job_output,job_kill,job_list",
        ])
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
    let head = text.lines().next().unwrap_or(text);
    let body = head
        .strip_prefix("=== job ")
        .and_then(|value| value.strip_suffix(" ==="))
        .unwrap_or_else(|| panic!("run_background must return a start head note; got {text:?}"));
    let (id, log) = body
        .split_once(" (started; log at ")
        .unwrap_or_else(|| panic!("run_background must return the log path; got {text:?}"));
    let log = log
        .strip_suffix(')')
        .unwrap_or_else(|| panic!("run_background start metric was not closed: {text:?}"));
    assert!(valid_job_id(id), "{id}");
    assert!(Path::new(log).is_absolute(), "{log}");
    id.to_string()
}

fn started_job_log(text: &str) -> String {
    let head = text.lines().next().unwrap_or(text);
    let body = head
        .strip_prefix("=== job ")
        .and_then(|value| value.strip_suffix(" ==="))
        .unwrap_or_else(|| panic!("invalid run_background head note: {text:?}"));
    let (_, log) = body
        .split_once(" (started; log at ")
        .unwrap_or_else(|| panic!("missing run_background log path: {text:?}"));
    log.strip_suffix(')')
        .unwrap_or_else(|| panic!("run_background start metric was not closed: {text:?}"))
        .to_string()
}

fn valid_job_id(id: &str) -> bool {
    id.len() == 8
        && id.starts_with("j-")
        && id[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

fn omitted_start(text: &str) -> u64 {
    let head = text.lines().next().unwrap_or(text);
    head.split("; lines ")
        .nth(1)
        .and_then(|range| range.split_once('-'))
        .and_then(|(first, _)| first.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("job_output did not report an omitted range: {text}"))
}

fn job_body_lines(text: &str) -> Vec<String> {
    let lines = text.lines().collect::<Vec<_>>();
    let start = 1 + usize::from(
        lines
            .get(1)
            .is_some_and(|line| line.starts_with("=== jobs:")),
    );
    lines[start..]
        .iter()
        .map(|line| (*line).to_string())
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

fn wait_for_terminal_from(
    session: &mut McpSession,
    job_id: &str,
    after_seq: Option<u64>,
) -> String {
    wait_for_terminal_from_within(session, job_id, after_seq, Duration::from_secs(15))
}

fn wait_for_terminal_from_within(
    session: &mut McpSession,
    job_id: &str,
    after_seq: Option<u64>,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
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
        if terminal_job_head(&text, job_id) {
            return text;
        }
    }
}

fn terminal_job_head(text: &str, job_id: &str) -> bool {
    let Some(head) = text.lines().next() else {
        return false;
    };
    let prefix = format!("=== job {job_id} ");
    head.starts_with(&prefix)
        && (head.contains(" exited ")
            || head.contains(" killed (")
            || head.contains(" interrupted ("))
}

fn background_line(text: &str) -> Option<&str> {
    text.lines().find(|line| line.starts_with("=== jobs:"))
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
            serde_json::json!({"job_id": job_id, "wait_ms": 0, "after_seq": 0}),
        );
        let text = mcp_text(&output).to_string();
        if text.contains(needle) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
struct OutputLogLock {
    file: std::fs::File,
    overlapped: Box<windows_sys::Win32::System::IO::OVERLAPPED>,
}

#[cfg(windows)]
impl Drop for OutputLogLock {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;

        // SAFETY: the file and OVERLAPPED storage outlive this call and describe
        // the same byte range locked by `lock_output_log`.
        unsafe {
            UnlockFileEx(
                self.file.as_raw_handle(),
                0,
                u32::MAX,
                u32::MAX,
                self.overlapped.as_mut(),
            );
        }
    }
}

#[cfg(windows)]
fn lock_output_log(path: &Path) -> OutputLogLock {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    // SAFETY: zero is the documented synchronous-file offset representation.
    let mut overlapped = Box::new(unsafe { std::mem::zeroed::<OVERLAPPED>() });
    // SAFETY: the handle is valid and both the file and OVERLAPPED storage stay
    // alive in the returned guard until the range is unlocked.
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            overlapped.as_mut(),
        )
    };
    assert_ne!(
        locked,
        0,
        "failed to lock {}: {}",
        normalized(path),
        std::io::Error::last_os_error()
    );
    OutputLogLock { file, overlapped }
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
