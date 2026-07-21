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
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

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
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpSession {
    pub fn start(mut command: Command) -> Self {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout,
            next_id: 1,
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
        loop {
            let value = self.read();
            if value["id"].as_i64() == Some(id) {
                return value;
            }
        }
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

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "MCP server closed stdout before replying");
        serde_json::from_str(&line).unwrap()
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
