//! Raw-byte-preserving stdio JSON-RPC client for an arbitrary FastCtx binary.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
type FrameResult = Result<Vec<u8>, String>;
type StdoutReader = (Receiver<FrameResult>, JoinHandle<Result<(), String>>);

#[derive(Debug)]
pub struct Invocation {
    pub stdin: Vec<u8>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: ExitStatus,
    pub is_error: bool,
    pub content_kind: String,
    pub text: String,
}

pub fn binary_version(binary: &Path, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run {} --version: {error}", binary.display()))?;
    let status = wait_for_exit(&mut child, timeout)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| "version stdout was not piped".to_string())?
        .read_to_end(&mut stdout)
        .map_err(|error| format!("cannot read version stdout: {error}"))?;
    child
        .stderr
        .take()
        .ok_or_else(|| "version stderr was not piped".to_string())?
        .read_to_end(&mut stderr)
        .map_err(|error| format!("cannot read version stderr: {error}"))?;
    if !status.success() {
        return Err(format!(
            "{} --version exited with {status}: {}",
            binary.display(),
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    String::from_utf8(stdout)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("version output is not UTF-8: {error}"))
}

pub fn invoke(
    binary: &Path,
    cwd: &Path,
    home: &Path,
    environment: &BTreeMap<String, String>,
    tool: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<Invocation, String> {
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_isolated_environment(&mut command, home);
    command.env("FASTCTX_NO_PARENT_WATCH", "1");
    for (name, _) in std::env::vars_os() {
        let name_text = name.to_string_lossy();
        if name_text.starts_with("FASTCTX_") {
            command.env_remove(name);
        }
    }
    command.env("FASTCTX_NO_PARENT_WATCH", "1");
    for (name, value) in environment {
        if !name.starts_with("FASTCTX_") {
            return Err(format!(
                "capture environment key is not scoped to FastCtx: {name}"
            ));
        }
        command.env(name, value);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot spawn {}: {error}", binary.display()))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "server stdin was not piped".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "server stdout was not piped".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "server stderr was not piped".to_string())?;
    let (stdout_rx, stdout_thread) = spawn_stdout_reader(stdout);
    let stderr_thread = std::thread::spawn(move || -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        BufReader::new(stderr)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot drain server stderr: {error}"))?;
        Ok(bytes)
    });

    let result = communicate(&mut child, &mut stdin, &stdout_rx, tool, arguments, timeout);
    drop(stdin);
    let exit_result = wait_for_exit(&mut child, timeout);
    let stdout_join = join_reader(stdout_thread, "stdout");
    let mut stdout_bytes = Vec::new();
    while let Ok(frame) = stdout_rx.try_recv() {
        stdout_bytes.extend_from_slice(&frame?);
    }
    let stderr_bytes = join_bytes(stderr_thread, "stderr");

    let (mut prefix, is_error, content_kind, text) = result?;
    prefix.stdout.extend_from_slice(&stdout_bytes);
    stdout_join?;
    let stderr = stderr_bytes?;
    let exit_status = exit_result?;
    if !exit_status.success() {
        return Err(format!(
            "server exited with {exit_status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(Invocation {
        stdin: prefix.stdin,
        stdout: prefix.stdout,
        stderr,
        exit_status,
        is_error,
        content_kind,
        text,
    })
}

fn configure_isolated_environment(command: &mut Command, home: &Path) {
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("LC_")
            || text.starts_with("GIT_")
            || text.starts_with("XDG_")
            || matches!(
                text.as_ref(),
                "LANG"
                    | "LANGUAGE"
                    | "TZ"
                    | "HOME"
                    | "USERPROFILE"
                    | "HOMEDRIVE"
                    | "HOMEPATH"
                    | "APPDATA"
                    | "LOCALAPPDATA"
                    | "TMPDIR"
                    | "TMP"
                    | "TEMP"
            )
        {
            command.env_remove(name);
        }
    }
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home)
        .env("LOCALAPPDATA", home)
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_CACHE_HOME", home)
        .env("XDG_DATA_HOME", home)
        .env("TMPDIR", home)
        .env("TMP", home)
        .env("TEMP", home)
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("TZ", "UTC")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb");
}

struct Transcript {
    stdin: Vec<u8>,
    stdout: Vec<u8>,
}

fn communicate(
    child: &mut Child,
    stdin: &mut ChildStdin,
    stdout: &Receiver<FrameResult>,
    tool: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<(Transcript, bool, String, String), String> {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "compat-v011-capture", "version": "1.0"}
        }
    });
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    let mut transcript = Transcript {
        stdin: Vec::new(),
        stdout: Vec::new(),
    };
    send_frame(stdin, &initialize, &mut transcript.stdin)?;
    let (initialize_response, initialize_raw) =
        receive_response(child, stdout, 1, timeout, &mut transcript)?;
    if initialize_response.get("error").is_some() {
        return Err(format!(
            "initialize failed: {}",
            String::from_utf8_lossy(&initialize_raw)
        ));
    }
    send_frame(stdin, &initialized, &mut transcript.stdin)?;
    send_frame(stdin, &call, &mut transcript.stdin)?;
    let (value, _) = receive_response(child, stdout, 2, timeout, &mut transcript)?;
    let result = value
        .get("result")
        .ok_or_else(|| format!("tools/call returned no result: {value}"))?;
    let is_error = result
        .get("isError")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let content = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("tools/call result has no content array: {result}"))?;
    if content.len() != 1 {
        return Err(format!(
            "compat corpus requires exactly one content block, got {}",
            content.len()
        ));
    }
    let content_kind = content[0]
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "content block has no type".to_string())?
        .to_string();
    let text = content[0]
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "content block is not text".to_string())?
        .to_string();
    Ok((transcript, is_error, content_kind, text))
}

fn receive_response(
    child: &mut Child,
    stdout: &Receiver<FrameResult>,
    expected_id: i64,
    timeout: Duration,
    transcript: &mut Transcript,
) -> Result<(serde_json::Value, Vec<u8>), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot poll server: {error}"))?
        {
            return Err(format!(
                "server exited with {status} before response id {expected_id}"
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("timed out waiting for response id {expected_id}"));
        }
        let frame = stdout
            .recv_timeout(remaining)
            .map_err(|error| format!("cannot receive response id {expected_id}: {error}"))??;
        transcript.stdout.extend_from_slice(&frame);
        let value: serde_json::Value = serde_json::from_slice(&frame)
            .map_err(|error| format!("server stdout frame is not JSON: {error}"))?;
        if value.get("id").and_then(serde_json::Value::as_i64) == Some(expected_id) {
            return Ok((value, frame));
        }
    }
}

fn send_frame(
    stdin: &mut ChildStdin,
    value: &serde_json::Value,
    transcript: &mut Vec<u8>,
) -> Result<(), String> {
    let mut frame = serde_json::to_vec(value)
        .map_err(|error| format!("cannot serialize JSON-RPC request: {error}"))?;
    frame.push(b'\n');
    stdin
        .write_all(&frame)
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("cannot write JSON-RPC request: {error}"))?;
    transcript.extend_from_slice(&frame);
    Ok(())
}

fn spawn_stdout_reader(stdout: impl Read + Send + 'static) -> StdoutReader {
    let (sender, receiver) = mpsc::channel();
    let handle = std::thread::spawn(move || -> Result<(), String> {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut frame = Vec::new();
            let read = reader
                .read_until(b'\n', &mut frame)
                .map_err(|error| format!("cannot read server stdout: {error}"))?;
            if read == 0 {
                return Ok(());
            }
            if frame.len() > MAX_FRAME_BYTES {
                return Err(format!(
                    "server stdout frame exceeds {MAX_FRAME_BYTES} bytes"
                ));
            }
            if sender.send(Ok(frame)).is_err() {
                return Ok(());
            }
        }
    });
    (receiver, handle)
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot poll child process: {error}"))?
        {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("child process did not exit before the capture deadline".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn join_reader(handle: JoinHandle<Result<(), String>>, name: &str) -> Result<(), String> {
    handle
        .join()
        .map_err(|_| format!("{name} reader thread panicked"))?
}

fn join_bytes(handle: JoinHandle<Result<Vec<u8>, String>>, name: &str) -> Result<Vec<u8>, String> {
    handle
        .join()
        .map_err(|_| format!("{name} reader thread panicked"))?
}
