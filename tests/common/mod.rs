//! Shared test helpers for fixtures generated at runtime.

#![allow(dead_code)]

use fastctx::glob_tool::GlobRequest;
use fastctx::grep_tool::GrepRequest;
use fastctx::{ToolContent, ToolResponse};
use filetime::{FileTime, set_file_mtime};
use lopdf::content::{Content, Operation};
use lopdf::{
    Document, EncryptionState, EncryptionVersion, Object, Permissions, Stream, dictionary,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Idle timeout every test-spawned control center must use.
///
/// A host keeps the test binary open on Windows for as long as it lives, so one that outlives its
/// suite makes the next `cargo test` invocation fail to relink. Any test that spawns the binary
/// outside [`McpSession::start`] has to set `FASTCTX_TEST_RUNTIME_IDLE_MS` to this value itself.
pub const TEST_HOST_IDLE_MS: &str = "5000";
const MCP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Gives every spawned fixture a private profile and OS cache/temp roots unless the caller
/// explicitly selected a value for that variable.
pub fn isolate_command(command: &mut Command, root: &Path) {
    let default_home = root.join("home");
    let home = command_environment_path(command, "HOME")
        .or_else(|| command_environment_path(command, "USERPROFILE"))
        .unwrap_or(default_home);
    let temp = root.join("temp");
    let local_app_data = root.join("local-app-data");
    let app_data = root.join("app-data");
    let xdg_runtime = root.join("xdg-runtime");
    let xdg_config = root.join("xdg-config");
    let xdg_cache = root.join("xdg-cache");
    let xdg_data = root.join("xdg-data");
    for path in [
        &home,
        &temp,
        &local_app_data,
        &app_data,
        &xdg_runtime,
        &xdg_config,
        &xdg_cache,
        &xdg_data,
    ] {
        fs::create_dir_all(path).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&xdg_runtime, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for (name, value) in [
        ("HOME", &home),
        ("USERPROFILE", &home),
        ("TMPDIR", &temp),
        ("TMP", &temp),
        ("TEMP", &temp),
        ("LOCALAPPDATA", &local_app_data),
        ("APPDATA", &app_data),
        ("XDG_RUNTIME_DIR", &xdg_runtime),
        ("XDG_CONFIG_HOME", &xdg_config),
        ("XDG_CACHE_HOME", &xdg_cache),
        ("XDG_DATA_HOME", &xdg_data),
    ] {
        if !command_has_environment_override(command, name) {
            command.env(name, value);
        }
    }
    if !command_has_environment_override(command, "CODEX_HOME") {
        command.env_remove("CODEX_HOME");
    }
}

fn command_has_environment_override(command: &Command, expected: &str) -> bool {
    command
        .get_envs()
        .any(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case(expected))
}

fn command_environment_path(command: &Command, expected: &str) -> Option<PathBuf> {
    command.get_envs().find_map(|(name, value)| {
        name.to_string_lossy()
            .eq_ignore_ascii_case(expected)
            .then(|| value.map(PathBuf::from))
            .flatten()
    })
}

pub fn text(response: ToolResponse) -> String {
    assert!(!response.is_error, "unexpected tool error: {response:?}");
    assert_eq!(response.content.len(), 1);
    match response.content.into_iter().next().unwrap() {
        ToolContent::Text(text) => text,
        content => panic!("expected text content, got {content:?}"),
    }
}

pub fn error_text(response: ToolResponse) -> String {
    assert!(response.is_error, "expected tool error: {response:?}");
    assert_eq!(response.content.len(), 1);
    match response.content.into_iter().next().unwrap() {
        ToolContent::Text(text) => text,
        content => panic!("expected text error, got {content:?}"),
    }
}

pub fn grep_files(request: GrepRequest) -> ToolResponse {
    fastctx::grep_tool::grep_files(request, tokio_util::sync::CancellationToken::new())
}

pub fn glob_files(request: GlobRequest) -> ToolResponse {
    fastctx::glob_tool::glob_files(request, tokio_util::sync::CancellationToken::new())
}

pub fn normalized(path: &Path) -> String {
    let absolute = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut value = absolute.to_string_lossy().replace('\\', "/");
    if let Some(rest) = value.strip_prefix("//?/UNC/") {
        value = format!("//{rest}");
    } else if let Some(rest) = value.strip_prefix("//?/") {
        value = rest.to_string();
    }
    value
}

pub fn cwd() -> String {
    normalized(&std::env::current_dir().unwrap())
}

pub fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

pub struct McpSession {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    responses: mpsc::Receiver<Result<String, String>>,
    pending_responses: BTreeMap<i64, Value>,
    stderr: Option<ChildStderr>,
    next_id: i64,
    _isolation: tempfile::TempDir,
}

impl McpSession {
    pub fn start(mut command: Command) -> Self {
        let isolation = tempfile::Builder::new()
            .prefix("fastctx-mcp-session-")
            .tempdir()
            .unwrap();
        isolate_command(&mut command, isolation.path());
        if !command
            .get_envs()
            .any(|(name, _)| name == "FASTCTX_TEST_RUNTIME_IDLE_MS")
        {
            command.env("FASTCTX_TEST_RUNTIME_IDLE_MS", TEST_HOST_IDLE_MS);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (response_sender, responses) = mpsc::channel();
        std::thread::spawn(move || read_responses(stdout, response_sender));
        let stderr = child.stderr.take();
        let mut session = Self {
            child: Some(child),
            stdin: Some(stdin),
            responses,
            pending_responses: BTreeMap::new(),
            stderr,
            next_id: 1,
            _isolation: isolation,
        };
        let initialized = session.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "contract-test", "version": "1.0"}
            }),
        );
        assert!(initialized.get("error").is_none(), "{initialized}");
        session.notify("notifications/initialized", serde_json::json!({}));
        session
    }

    pub fn list_tools(&mut self) -> Vec<String> {
        let response = self.request("tools/list", serde_json::json!({}));
        response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            serde_json::json!({"name": name, "arguments": arguments}),
        )
    }

    pub fn call_with_timeout(&mut self, name: &str, arguments: Value, timeout: Duration) -> Value {
        let id = self.begin_call(name, arguments);
        self.await_response_with_timeout(id, timeout)
    }

    pub fn begin_call(&mut self, name: &str, arguments: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }));
        id
    }

    pub fn await_response(&mut self, id: i64) -> Value {
        self.await_response_with_timeout(id, MCP_RESPONSE_TIMEOUT)
    }

    pub fn await_response_with_timeout(&mut self, id: i64, timeout: Duration) -> Value {
        if let Some(response) = self.pending_responses.remove(&id) {
            return response;
        }
        loop {
            let value = self.read_with_timeout(timeout);
            if value["id"].as_i64() == Some(id) {
                return value;
            }
            if let Some(other_id) = value["id"].as_i64() {
                self.pending_responses.insert(other_id, value);
            }
        }
    }

    pub fn child_id(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    pub fn disconnect_stdin(&mut self) {
        self.stdin.take();
    }

    pub fn kill_proxy(mut self) -> ExitStatus {
        self.stdin.take();
        let mut child = self.child.take().unwrap();
        let _ = child.kill();
        child.wait().unwrap()
    }

    pub fn kill_proxy_with_stderr(mut self) -> (ExitStatus, String) {
        self.stdin.take();
        let mut child = self.child.take().unwrap();
        let _ = child.kill();
        let status = child.wait().unwrap();
        let mut stderr = String::new();
        if let Some(mut pipe) = self.stderr.take() {
            pipe.read_to_string(&mut stderr).unwrap();
        }
        (status, stderr)
    }

    pub fn wait_for_exit_with_stderr(mut self) -> (ExitStatus, String) {
        let mut child = self.child.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let status = child.wait().unwrap();
                panic!(
                    "MCP server did not exit after its control center closed; killed with {status}"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        self.stdin.take();
        let mut stderr = String::new();
        if let Some(mut pipe) = self.stderr.take() {
            pipe.read_to_string(&mut stderr).unwrap();
        }
        (status, stderr)
    }

    pub fn close(mut self) -> ExitStatus {
        self.stdin.take();
        let mut child = self.child.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let status = child.wait().unwrap();
                panic!("MCP server did not exit after stdin closed; killed with {status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        self.await_response(id)
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    fn send(&mut self, value: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::to_string(&value).unwrap()).unwrap();
        stdin.flush().unwrap();
    }

    fn read_with_timeout(&mut self, timeout: Duration) -> Value {
        let line = self
            .responses
            .recv_timeout(timeout)
            .unwrap_or_else(|error| panic!("MCP server did not reply within {timeout:?}: {error}"))
            .unwrap_or_else(|error| panic!("MCP server stdout failed: {error}"));
        serde_json::from_str(&line).unwrap()
    }
}

fn read_responses(stdout: ChildStdout, sender: mpsc::Sender<Result<String, String>>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(Err("MCP server closed stdout before replying".to_string()));
                return;
            }
            Ok(_) => {
                if sender.send(Ok(line)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
        }
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn mcp_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"].as_str().unwrap()
}

pub fn set_mtime(path: &Path, seconds: i64) {
    set_file_mtime(path, FileTime::from_unix_time(seconds, 0)).unwrap();
}

pub fn write_pdf(path: &Path, page_texts: &[Option<&str>]) {
    write_pdf_with_media_box(path, page_texts, 595, 842);
}

pub fn write_pdf_with_media_box(
    path: &Path,
    page_texts: &[Option<&str>],
    width_points: i64,
    height_points: i64,
) {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Courier",
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let mut page_ids = Vec::with_capacity(page_texts.len());
    for page_text in page_texts {
        let operations = page_text.map_or_else(Vec::new, |text| {
            vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
            ]
        });
        let content = Content { operations }.encode().unwrap();
        let content_id = document.add_object(Stream::new(dictionary! {}, content));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        page_ids.push(page_id.into());
    }
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => page_ids,
            "Count" => page_texts.len() as i64,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), width_points.into(), height_points.into()],
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    document.trailer.set(
        "ID",
        Object::Array(vec![
            Object::String((1_u8..=16).collect(), lopdf::StringFormat::Literal),
            Object::String((1_u8..=16).rev().collect(), lopdf::StringFormat::Literal),
        ]),
    );
    document.compress();
    document.save(path).unwrap();
}

pub fn write_encrypted_pdf(path: &Path) {
    let plain = path.with_extension("plain.pdf");
    write_pdf(&plain, &[Some("Secret")]);
    let mut document = Document::load(&plain).unwrap();
    let version = EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner-password",
        user_password: "user-password",
        key_length: 128,
        permissions: Permissions::PRINTABLE,
    };
    let state = EncryptionState::try_from(version).unwrap();
    document.encrypt(&state).unwrap();
    document.save(path).unwrap();
    fs::remove_file(plain).unwrap();
}
