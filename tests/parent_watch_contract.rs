mod common;

use common::{McpSession, TEST_HOST_IDLE_MS, isolate_command, mcp_text, normalized};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Poll-until budget for "a process this suite started has reached the state we need".
///
/// No promise is bounded here — every promptness claim in this file has its own constant —
/// so widening this only delays a failure report, while narrowing it reports a busy machine
/// as a defect. The chain it waits on is long: a cold control center from an empty HOME, then
/// a shell fixture that starts Windows PowerShell to publish its own PID.
///
/// The Windows ARM64 runner missed 30 s and then 45 s (observed 2026-09-04 in CI); no other
/// target has ever come close. Whether that machine is merely that slow to start PowerShell
/// cold is not yet settled — `wait_for_pid_file` now reports what the server had answered and
/// what the fixture directory held, so the next miss says which it is.
const PROCESS_DEADLINE: Duration = Duration::from_secs(90);
const IDLE_PROBE: Duration = Duration::from_millis(1_500);
/// Upper bound on the promptness this suite claims for an EOF-triggered shutdown.
///
/// This one is load-bearing: an in-flight request must not outlive stdin EOF, and the window
/// has to stay tight enough to catch a shutdown that quietly waits out a drain instead.
const EOF_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(2);
/// Poll-until budget for a serve that must fail on an unreadable stdin.
///
/// Nothing the product promises is bounded here — the assertion is that it exits nonzero at
/// all. The run starts a cold control center from an empty HOME, which costs over a second on
/// an idle machine and more inside a full test group, so a tight budget would only report a
/// busy machine as a defect (observed 2026-08-08 in the local gate).
#[cfg(windows)]
const STARTUP_FAILURE_DEADLINE: Duration = Duration::from_secs(30);

static PARENT_WATCH_SUITE_LOCK: Mutex<()> = Mutex::new(());

fn parent_watch_suite_guard() -> MutexGuard<'static, ()> {
    // These tests intentionally terminate control centers and process trees. Running their
    // lifecycle fixtures concurrently loses owed EOF responses on Windows under ordinary suite
    // load, while each contract is stable in isolation (observed 2026-08-29).
    PARENT_WATCH_SUITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn parent_watch_exits_without_stdin_eof_but_preserves_live_and_opted_out_servers() {
    let _suite = parent_watch_suite_guard();
    let temp = tempfile::tempdir().unwrap();

    let watched = spawn_through_short_lived_parent(temp.path(), "watched", false);
    wait_for_process_exit(&watched.process, PROCESS_DEADLINE);
    drop(watched.stdin_writer);

    let escaped = spawn_through_short_lived_parent(temp.path(), "escaped", true);
    std::thread::sleep(IDLE_PROBE);
    assert!(
        process_is_alive(&escaped.process),
        "FASTCTX_NO_PARENT_WATCH=1 must preserve the server while stdin remains open"
    );
    terminate_process(&escaped.process);
    wait_for_process_exit(&escaped.process, PROCESS_DEADLINE);
    drop(escaped.stdin_writer);

    let (stdin_reader, mut stdin_writer) = anonymous_pipe();
    let response = temp.path().join("live-parent-response.jsonl");
    let output = File::create(&response).unwrap();
    let mut live = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    isolate_command(&mut live, temp.path());
    let mut live = live
        .arg("serve")
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", TEST_HOST_IDLE_MS)
        .env_remove("FASTCTX_NO_PARENT_WATCH")
        .stdin(Stdio::from(stdin_reader))
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    write_initialize(&mut stdin_writer);
    wait_for_nonempty_file(&response, PROCESS_DEADLINE);
    std::thread::sleep(IDLE_PROBE);
    assert!(
        live.try_wait().unwrap().is_none(),
        "a live parent and idle stdin must not trigger shutdown"
    );
    live.kill().unwrap();
    live.wait().unwrap();
}

#[test]
fn parent_watch_ends_foreground_work_but_preserves_detached_background_jobs() {
    let _suite = parent_watch_suite_guard();
    let temp = tempfile::tempdir().unwrap();
    let background_root = temp.path().join("background");
    std::fs::create_dir(&background_root).unwrap();
    let mut background = spawn_controlled_parent(&background_root, "background", true);
    initialize_controlled_server(&mut background);
    send_json(
        &mut background.stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "run_background",
                "arguments": {
                    "command": "sleep 1; printf 'survived-parent-watch\\n'; exit 17",
                    "login_shell": false
                }
            }
        }),
    );
    let started = wait_for_response(&background.response_path, 2, PROCESS_DEADLINE);
    let body = mcp_text(&started)
        .strip_prefix("=== job ")
        .and_then(|value| value.strip_suffix(" ==="))
        .expect("run_background must return its durable start head note");
    let (job_id, log_path) = body
        .split_once(" (started; log at ")
        .expect("run_background must return its durable job id and log path");
    let log_path = log_path
        .strip_suffix(')')
        .expect("run_background start facts must close their head-note metric");
    assert!(Path::new(log_path).is_absolute(), "{log_path}");
    let job_id = job_id.to_string();
    release_parent_and_wait_for_server(&mut background);

    let mut command = shell_server_command(&background_root);
    command.env("FASTCTX_NO_PARENT_WATCH", "1");
    let mut replacement = McpSession::start(command);
    let completion_deadline = Instant::now() + PROCESS_DEADLINE;
    let output = loop {
        assert!(
            Instant::now() < completion_deadline,
            "detached background job {job_id} never reached a terminal state"
        );
        let response = replacement.call(
            "job_output",
            serde_json::json!({"job_id": job_id, "wait_ms": 2_000, "after_seq": 0}),
        );
        let output = mcp_text(&response).to_string();
        if output
            .lines()
            .next()
            .is_some_and(|line| line.starts_with(&format!("=== job {job_id} exited 17 (")))
        {
            break output;
        }
    };
    assert!(output.contains("survived-parent-watch"), "{output}");
    assert!(output.contains("exited 17"), "{output}");
    assert!(replacement.close().success());

    let foreground_root = temp.path().join("foreground");
    std::fs::create_dir(&foreground_root).unwrap();
    let pid_path = foreground_root.join("foreground.pid");
    let escaped_marker = foreground_root.join("escaped.txt");
    let foreground_command = foreground_fixture_command(&pid_path, &escaped_marker);
    let mut foreground = spawn_controlled_parent(&foreground_root, "foreground", true);
    initialize_controlled_server(&mut foreground);
    send_json(
        &mut foreground.stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "run",
                "arguments": {
                    "command": foreground_command,
                    "login_shell": false,
                    "timeout_ms": 60_000
                }
            }
        }),
    );
    let foreground_pid =
        wait_for_pid_file(&pid_path, PROCESS_DEADLINE, Some(&foreground.response_path));
    let foreground_process = ProcessProbe::capture(foreground_pid);
    release_parent_and_wait_for_server(&mut foreground);
    wait_for_process_exit(&foreground_process, PROCESS_DEADLINE);
    assert!(
        !escaped_marker.exists(),
        "foreground work must not outlive the server that owns its response"
    );
}

#[test]
fn stdin_eof_ends_inflight_foreground_work_promptly() {
    let _suite = parent_watch_suite_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let temp_dir = root.join("tmp");
    std::fs::create_dir(&temp_dir).unwrap();
    let pid_path = root.join("foreground.pid");
    let escaped_marker = root.join("escaped.txt");
    let foreground_command = foreground_fixture_command(&pid_path, &escaped_marker);
    let response_path = root.join("response.jsonl");
    let output = File::create(&response_path).unwrap();
    let (stdin_reader, mut stdin_writer) = anonymous_pipe();
    let mut server = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    isolate_command(&mut server, root);
    let mut server = server
        .args(["serve", "--enable-shell"])
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("TMPDIR", &temp_dir)
        .env("TMP", &temp_dir)
        .env("TEMP", &temp_dir)
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", TEST_HOST_IDLE_MS)
        .env_remove("FASTCTX_NO_PARENT_WATCH")
        .stdin(Stdio::from(stdin_reader))
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    write_initialize(&mut stdin_writer);
    wait_for_nonempty_file(&response_path, PROCESS_DEADLINE);
    send_json(
        &mut stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    );
    send_json(
        &mut stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "run",
                "arguments": {
                    "command": foreground_command,
                    "login_shell": false,
                    "timeout_ms": 60_000
                }
            }
        }),
    );
    let foreground_pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE, Some(&response_path));
    let foreground = ProcessProbe::capture(foreground_pid);

    let eof_started = Instant::now();
    drop(stdin_writer);
    let Some(status) = wait_for_child_exit(&mut server, EOF_SHUTDOWN_DEADLINE) else {
        let _ = server.kill();
        let _ = server.wait();
        terminate_process(&foreground);
        panic!(
            "serve did not exit within {:?} after stdin EOF with an in-flight request",
            EOF_SHUTDOWN_DEADLINE
        );
    };
    let eof_delay = eof_started.elapsed();
    assert!(status.success(), "serve failed after stdin EOF: {status}");
    assert!(
        eof_delay < EOF_SHUTDOWN_DEADLINE,
        "serve took {eof_delay:?} to exit after stdin EOF"
    );
    wait_for_process_exit(&foreground, PROCESS_DEADLINE);
    assert!(
        !escaped_marker.exists(),
        "in-flight foreground work must not outlive stdin EOF"
    );
}

/// The counterweight to the promptness contract above: ending a session must not cost the answers
/// the server already owes. `initialize | tools/list | close stdin` is how a script or a smoke test
/// drives an MCP server, and a proxy that abandons the connection at EOF answers none of it while
/// still exiting successfully.
#[test]
fn stdin_eof_still_answers_requests_that_were_already_sent() {
    let _suite = parent_watch_suite_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let response_path = root.join("response.jsonl");
    let diagnostics_path = root.join("stderr.txt");
    let output = File::create(&response_path).unwrap();
    let diagnostics = File::create(&diagnostics_path).unwrap();
    let (stdin_reader, mut stdin_writer) = anonymous_pipe();
    let mut server = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    isolate_command(&mut server, root);
    let mut server = server
        .arg("serve")
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", TEST_HOST_IDLE_MS)
        .env_remove("FASTCTX_NO_PARENT_WATCH")
        .stdin(Stdio::from(stdin_reader))
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(diagnostics))
        .spawn()
        .unwrap();

    // Everything this client will ever say, then EOF, with no read in between.
    write_initialize(&mut stdin_writer);
    send_json(
        &mut stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    );
    send_json(
        &mut stdin_writer,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    );
    let eof_started = Instant::now();
    drop(stdin_writer);

    let Some(status) = wait_for_child_exit(&mut server, PROCESS_DEADLINE) else {
        let _ = server.kill();
        let _ = server.wait();
        panic!("serve did not exit within {PROCESS_DEADLINE:?} after stdin EOF");
    };
    let eof_delay = eof_started.elapsed();
    assert!(status.success(), "serve failed after stdin EOF: {status}");

    let answers = std::fs::read_to_string(&response_path).unwrap();
    let answered = answers
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|message| message.get("id").and_then(Value::as_u64))
        .collect::<Vec<_>>();
    let diagnostics = std::fs::read_to_string(&diagnostics_path).unwrap_or_default();
    assert!(
        answered.contains(&1) && answered.contains(&2),
        "stdin EOF discarded answers that were already owed after {eof_delay:?}; \
         answered {answered:?}, stdout {answers:?}, stderr {diagnostics:?}"
    );
}

#[cfg(windows)]
#[test]
fn stdin_startup_read_error_is_not_reported_as_clean_eof() {
    use std::io::Read;

    let _suite = parent_watch_suite_guard();
    let temp = tempfile::tempdir().unwrap();
    let unreadable_stdin = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(temp.path().join("write-only-stdin"))
        .unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    isolate_command(&mut server, temp.path());
    let mut server = server
        .arg("serve")
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", TEST_HOST_IDLE_MS)
        .env_remove("FASTCTX_NO_PARENT_WATCH")
        .stdin(Stdio::from(unreadable_stdin))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let Some(status) = wait_for_child_exit(&mut server, STARTUP_FAILURE_DEADLINE) else {
        let _ = server.kill();
        let _ = server.wait();
        panic!(
            "serve did not report a startup stdin read error within {:?}",
            STARTUP_FAILURE_DEADLINE
        );
    };
    let mut stderr = String::new();
    server
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(
        !status.success(),
        "stdin read error was reported as success"
    );
    assert!(
        stderr.contains("Cannot read MCP stdin:"),
        "missing stdin read diagnostic: {stderr:?}"
    );
}

struct SpawnedServer {
    process: ProcessProbe,
    stdin_writer: File,
}

struct ControlledServer {
    process: ProcessProbe,
    helper: Child,
    stdin_writer: File,
    response_path: PathBuf,
    release_path: PathBuf,
}

fn spawn_controlled_parent(root: &Path, label: &str, enable_shell: bool) -> ControlledServer {
    let pid_path = root.join(format!("{label}-pid"));
    let response_path = root.join(format!("{label}-response.jsonl"));
    let release_path = root.join(format!("{label}-release"));
    let temp = root.join("tmp");
    std::fs::create_dir_all(&temp).unwrap();
    let (stdin_reader, stdin_writer) = anonymous_pipe();
    let mut helper = Command::new(std::env::current_exe().unwrap());
    isolate_command(&mut helper, root);
    helper
        .args([
            "--ignored",
            "--exact",
            "parent_watch_fixture_parent",
            "--nocapture",
        ])
        .env("FASTCTX_WATCH_FIXTURE_PID", &pid_path)
        .env("FASTCTX_WATCH_FIXTURE_RESPONSE", &response_path)
        .env("FASTCTX_WATCH_FIXTURE_RELEASE", &release_path)
        .env(
            "FASTCTX_WATCH_FIXTURE_ENABLE_SHELL",
            if enable_shell { "1" } else { "0" },
        )
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp)
        .env_remove("FASTCTX_NO_PARENT_WATCH")
        .stdin(Stdio::from(stdin_reader))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let helper = helper.spawn().unwrap();
    let helper_pid = helper.id();
    let pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE, None);
    assert_eq!(
        direct_parent_pid(pid),
        Some(helper_pid),
        "the controlled fixture must make its helper the server's direct parent"
    );
    ControlledServer {
        process: ProcessProbe::capture(pid),
        helper,
        stdin_writer,
        response_path,
        release_path,
    }
}

fn initialize_controlled_server(server: &mut ControlledServer) {
    send_json(
        &mut server.stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "parent-watch-contract", "version": "1.0"}
            }
        }),
    );
    let initialized = wait_for_response(&server.response_path, 1, PROCESS_DEADLINE);
    assert!(initialized.get("error").is_none(), "{initialized}");
    send_json(
        &mut server.stdin_writer,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    );
}

fn release_parent_and_wait_for_server(server: &mut ControlledServer) {
    std::fs::write(&server.release_path, b"exit").unwrap();
    let status = wait_for_child(&mut server.helper, PROCESS_DEADLINE);
    assert!(status.success(), "fixture parent failed: {status}");
    wait_for_process_exit(&server.process, PROCESS_DEADLINE);
}

fn shell_server_command(root: &Path) -> Command {
    let temp = root.join("tmp");
    std::fs::create_dir_all(&temp).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    isolate_command(&mut command, root);
    command
        .args(["serve", "--enable-shell"])
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("TMPDIR", &temp)
        .env("TMP", &temp)
        .env("TEMP", &temp);
    command
}

fn send_json(writer: &mut File, value: Value) {
    writeln!(writer, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    writer.flush().unwrap();
}

fn wait_for_response(path: &Path, id: i64, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(source) = std::fs::read_to_string(path) {
            for line in source.lines() {
                if let Ok(value) = serde_json::from_str::<Value>(line)
                    && value["id"].as_i64() == Some(id)
                {
                    return value;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("MCP server did not answer request {id}");
}

fn bash_quote(path: &Path) -> String {
    format!("'{}'", normalized(path).replace('\'', "'\\''"))
}

#[cfg(unix)]
fn foreground_fixture_command(pid_path: &Path, escaped_marker: &Path) -> String {
    format!(
        "printf '%s' \"$$\" > {}; sleep 30; printf escaped > {}",
        bash_quote(pid_path),
        bash_quote(escaped_marker)
    )
}

#[cfg(windows)]
fn foreground_fixture_command(pid_path: &Path, escaped_marker: &Path) -> String {
    let script_path = pid_path.with_extension("ps1");
    let powershell_quote = |path: &Path| path.to_string_lossy().replace('\'', "''");
    std::fs::write(
        &script_path,
        format!(
            "[IO.File]::WriteAllText('{}', $PID.ToString())\nStart-Sleep -Seconds 30\n[IO.File]::WriteAllText('{}', 'escaped')\n",
            powershell_quote(pid_path),
            powershell_quote(escaped_marker)
        ),
    )
    .unwrap();
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {}",
        bash_quote(&script_path)
    )
}

fn spawn_through_short_lived_parent(
    root: &Path,
    label: &str,
    disable_watch: bool,
) -> SpawnedServer {
    let pid_path = root.join(format!("{label}-pid"));
    let response_path = root.join(format!("{label}-response.jsonl"));
    let (stdin_reader, mut stdin_writer) = anonymous_pipe();
    let mut helper = Command::new(std::env::current_exe().unwrap());
    isolate_command(&mut helper, root);
    helper
        .args([
            "--ignored",
            "--exact",
            "parent_watch_fixture_parent",
            "--nocapture",
        ])
        .env("FASTCTX_WATCH_FIXTURE_PID", &pid_path)
        .env("FASTCTX_WATCH_FIXTURE_RESPONSE", &response_path)
        .stdin(Stdio::from(stdin_reader))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if disable_watch {
        helper.env("FASTCTX_NO_PARENT_WATCH", "1");
    } else {
        helper.env_remove("FASTCTX_NO_PARENT_WATCH");
    }
    let mut helper = helper.spawn().unwrap();
    let helper_pid = helper.id();
    let pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE, None);
    assert_eq!(
        direct_parent_pid(pid),
        Some(helper_pid),
        "the fixture must make the short-lived helper the server's direct parent"
    );
    let process = ProcessProbe::capture(pid);
    write_initialize(&mut stdin_writer);
    wait_for_nonempty_file(&response_path, PROCESS_DEADLINE);
    let status = wait_for_child(&mut helper, PROCESS_DEADLINE);
    assert!(status.success(), "fixture parent failed: {status}");
    SpawnedServer {
        process,
        stdin_writer,
    }
}

#[test]
#[ignore]
#[allow(clippy::zombie_processes)]
fn parent_watch_fixture_parent() {
    let pid_path = required_path("FASTCTX_WATCH_FIXTURE_PID");
    let response_path = required_path("FASTCTX_WATCH_FIXTURE_RESPONSE");
    let output = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&response_path)
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastctx"));
    command
        .arg("serve")
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", TEST_HOST_IDLE_MS);
    if std::env::var("FASTCTX_WATCH_FIXTURE_ENABLE_SHELL")
        .ok()
        .as_deref()
        == Some("1")
    {
        command.arg("--enable-shell");
    }
    // This helper must exit without waiting so the child can observe its direct parent's death.
    let mut server = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::fs::write(&pid_path, server.id().to_string()).unwrap();
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while Instant::now() < deadline {
        if std::fs::metadata(&response_path)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
        {
            break;
        }
        if let Some(status) = server.try_wait().unwrap() {
            panic!("fixture server exited before initialization: {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if std::fs::metadata(&response_path)
        .map(|metadata| metadata.len() == 0)
        .unwrap_or(true)
    {
        panic!("fixture server did not initialize");
    }
    if let Some(release_path) = std::env::var_os("FASTCTX_WATCH_FIXTURE_RELEASE").map(PathBuf::from)
    {
        while Instant::now() < deadline {
            if release_path.exists() {
                return;
            }
            if let Some(status) = server.try_wait().unwrap() {
                panic!("fixture server exited before parent release: {status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture parent release was not requested");
    }
}

fn required_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} is required"))
}

fn write_initialize(writer: &mut File) {
    writeln!(
        writer,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "parent-watch-contract", "version": "1.0"}
            }
        })
    )
    .unwrap();
    writer.flush().unwrap();
}

/// Waits for a process this suite started to publish its PID.
///
/// `transcript` is the file the server's answers were written to, when the caller kept one. A
/// timeout here is otherwise mute — it cannot distinguish "the machine is slow" from "the call
/// came back as an error" — and this suite runs on runners no developer can reproduce.
fn wait_for_pid_file(path: &Path, timeout: Duration, transcript: Option<&Path>) -> u32 {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(value) = std::fs::read_to_string(path)
            && let Ok(pid) = value.parse()
        {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let answers = transcript
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.chars().take(4000).collect::<String>())
        .unwrap_or_else(|| "<no transcript kept>".to_string());
    // The listing separates "the command never started" from "it started and wrote nothing":
    // the fixture script is written before the call, its output file only by the command.
    let listing = path
        .parent()
        .and_then(|parent| std::fs::read_dir(parent).ok())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "<unreadable>".to_string());
    panic!(
        "no PID at {} within {timeout:?}; that directory holds [{listing}]; the server answered:\n{answers}",
        path.display()
    );
}

fn wait_for_nonempty_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::fs::metadata(path)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("MCP server did not answer initialize");
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_process_exit(process: &ProcessProbe, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_is_alive(process) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    terminate_process(process);
    panic!(
        "server PID {} did not exit after its parent died",
        process.pid()
    );
}

#[cfg(target_os = "linux")]
fn direct_parent_pid(pid: u32) -> Option<u32> {
    let source = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    source
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(target_os = "macos")]
fn direct_parent_pid(pid: u32) -> Option<u32> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid="])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(windows)]
fn direct_parent_pid(pid: u32) -> Option<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot is closed before returning.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: the entry size is initialized as required by Toolhelp.
    let mut present = unsafe { Process32FirstW(snapshot, &mut entry) };
    let mut parent = None;
    while present != 0 {
        if entry.th32ProcessID == pid {
            parent = Some(entry.th32ParentProcessID);
            break;
        }
        // SAFETY: snapshot and entry remain valid throughout enumeration.
        present = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    // SAFETY: this function owns the snapshot.
    unsafe {
        CloseHandle(snapshot);
    }
    parent
}

fn anonymous_pipe() -> (File, File) {
    // std::io::pipe keeps both ends out of spawned children (CLOEXEC / non-inheritable), so the
    // test process holds the sole writer and dropping it delivers EOF. A raw libc::pipe leaked
    // the write end into the server itself, which made stdin EOF undeliverable (2026-07-22).
    let (reader, writer) = std::io::pipe().expect("anonymous pipe");
    #[cfg(unix)]
    {
        use std::os::fd::OwnedFd;
        (
            File::from(OwnedFd::from(reader)),
            File::from(OwnedFd::from(writer)),
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::OwnedHandle;
        (
            File::from(OwnedHandle::from(reader)),
            File::from(OwnedHandle::from(writer)),
        )
    }
}

#[cfg(unix)]
struct ProcessProbe {
    pid: u32,
    started: String,
}

#[cfg(unix)]
impl ProcessProbe {
    fn capture(pid: u32) -> Self {
        Self {
            pid,
            started: process_start_token(pid)
                .unwrap_or_else(|| panic!("cannot capture process identity for PID {pid}")),
        }
    }

    fn pid(&self) -> u32 {
        self.pid
    }
}

#[cfg(unix)]
fn process_is_alive(process: &ProcessProbe) -> bool {
    process_start_token(process.pid).as_deref() == Some(process.started.as_str())
}

#[cfg(target_os = "linux")]
fn process_start_token(pid: u32) -> Option<String> {
    let source = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    source
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_string)
}

#[cfg(not(target_os = "linux"))]
#[cfg(unix)]
fn process_start_token(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !token.is_empty()).then_some(token)
}

#[cfg(windows)]
struct ProcessProbe {
    pid: u32,
    handle: std::os::windows::io::OwnedHandle,
}

#[cfg(windows)]
impl ProcessProbe {
    fn capture(pid: u32) -> Self {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
        use windows_sys::Win32::System::Threading::OpenProcess;

        // SAFETY: a successful call returns a new handle transferred to OwnedHandle.
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        assert!(
            !handle.is_null(),
            "cannot open process probe for PID {pid}: {}",
            std::io::Error::last_os_error()
        );
        Self {
            pid,
            // SAFETY: OpenProcess returned a newly owned process handle.
            handle: unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.cast()) },
        }
    }

    fn pid(&self) -> u32 {
        self.pid
    }
}

#[cfg(windows)]
fn process_is_alive(process: &ProcessProbe) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    // SAFETY: the retained process handle was opened with SYNCHRONIZE.
    (unsafe { WaitForSingleObject(process.handle.as_raw_handle().cast(), 0) }) != WAIT_OBJECT_0
}

#[cfg(unix)]
fn terminate_process(process: &ProcessProbe) {
    if process_is_alive(process) {
        // SAFETY: the identity probe still matches the fixture PID owned by this test.
        unsafe {
            libc::kill(process.pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn terminate_process(process: &ProcessProbe) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    if !process_is_alive(process) {
        return;
    }
    // The exact retained handle is still unsignalled, so its PID cannot have been reused.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, process.pid);
        if !process.is_null() {
            TerminateProcess(process, 1);
            CloseHandle(process);
        }
    }
}
