mod common;

use common::{McpSession, mcp_text, normalized};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROCESS_DEADLINE: Duration = Duration::from_secs(10);
const IDLE_PROBE: Duration = Duration::from_millis(1_500);
const EOF_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(2);

#[test]
fn parent_watch_exits_without_stdin_eof_but_preserves_live_and_opted_out_servers() {
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
    let mut live = Command::new(env!("CARGO_BIN_EXE_fastctx"))
        .arg("serve")
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
    let job_id = mcp_text(&started)
        .strip_prefix("(Complete: job ")
        .and_then(|value| value.strip_suffix(" started.)"))
        .expect("run_background must return its durable job id")
        .to_string();
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
            .last()
            .is_some_and(|line| line.starts_with("(Complete:"))
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
    let foreground_pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE);
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
    let mut server = Command::new(env!("CARGO_BIN_EXE_fastctx"))
        .args(["serve", "--enable-shell"])
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("TMPDIR", &temp_dir)
        .env("TMP", &temp_dir)
        .env("TEMP", &temp_dir)
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
    let foreground_pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE);
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

#[cfg(windows)]
#[test]
fn stdin_startup_read_error_is_not_reported_as_clean_eof() {
    let temp = tempfile::tempdir().unwrap();
    let unreadable_stdin = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(temp.path().join("write-only-stdin"))
        .unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_fastctx"))
        .arg("serve")
        .env_remove("FASTCTX_NO_PARENT_WATCH")
        .stdin(Stdio::from(unreadable_stdin))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let Some(status) = wait_for_child_exit(&mut server, EOF_SHUTDOWN_DEADLINE) else {
        let _ = server.kill();
        let _ = server.wait();
        panic!(
            "serve did not report a startup stdin read error within {:?}",
            EOF_SHUTDOWN_DEADLINE
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
    let pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE);
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
    let pid = wait_for_pid_file(&pid_path, PROCESS_DEADLINE);
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
    command.arg("serve");
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

fn wait_for_pid_file(path: &Path, timeout: Duration) -> u32 {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(value) = std::fs::read_to_string(path)
            && let Ok(pid) = value.parse()
        {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("fixture parent did not publish a server PID");
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

#[cfg(unix)]
fn anonymous_pipe() -> (File, File) {
    use std::os::fd::FromRawFd;

    let mut descriptors = [0; 2];
    // SAFETY: pipe initializes both descriptors, which are immediately transferred to File.
    assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
    // SAFETY: successful pipe returned two newly owned descriptors.
    unsafe {
        (
            File::from_raw_fd(descriptors[0]),
            File::from_raw_fd(descriptors[1]),
        )
    }
}

#[cfg(windows)]
fn anonymous_pipe() -> (File, File) {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: output pointers and the security attributes remain valid for the call.
    assert_ne!(
        unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) },
        0
    );
    // The fixture parent must inherit only the read side; the test keeps the sole writer open.
    // SAFETY: write is a valid pipe handle returned by CreatePipe.
    assert_ne!(
        unsafe { SetHandleInformation(write, HANDLE_FLAG_INHERIT, 0) },
        0
    );
    // SAFETY: successful CreatePipe returned two newly owned handles.
    unsafe {
        (
            File::from_raw_handle(read.cast()),
            File::from_raw_handle(write.cast()),
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
