//! `fastctx serve`: the MCP server an agent session starts.
//!
//! It speaks MCP's stdio transport with the host and turns each tool call
//! into a call to this machine's node, starting the node if it is not
//! running. It keeps nothing of the World itself: when the node restarts,
//! calls in flight fail and say that whether they took effect is unknown;
//! the session goes on and the next call reaches the new node.

mod session;
mod stdio;
mod tools;

use std::collections::HashMap;
use std::sync::Arc;

use fastctx_types::layout::WorldDir;
use serde_json::{Value, json};
use tokio::task::{AbortHandle, JoinSet};

use session::Session;
use stdio::{Input, Output};
pub use tools::Writer;

/// The MCP revisions this server speaks; it answers in the client's when it
/// knows it, and in the newest otherwise, as the protocol asks.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Serves one session until its host closes standard input, then lets the
/// calls in flight answer. Each call ends within the `run` wait, so this
/// ends too.
pub async fn serve(world: WorldDir, writer: Writer) -> anyhow::Result<()> {
    let session = Arc::new(Session::new(world)?);
    let mut input = stdio::input();
    let (output, printing) = Output::start();
    let mut calls = JoinSet::new();
    let mut in_flight: HashMap<String, AbortHandle> = HashMap::new();
    while let Some(input) = input.recv().await {
        let line = match input {
            Input::Line(line) => line,
            Input::Failed(error) => {
                eprintln!("fastctx serve: reading standard input: {error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                output.send(&error_response(&Value::Null, -32700, &format!("parse error: {error}")));
                continue;
            }
        };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            (Some("initialize"), Some(id)) => {
                session.introduce(params.get("clientInfo").unwrap_or(&Value::Null));
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked.filter(|asked| PROTOCOL_VERSIONS.contains(asked)).unwrap_or(PROTOCOL_VERSIONS[0]);
                output.send(&response(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "fastctx", "version": env!("CARGO_PKG_VERSION") }
                    }),
                ));
            }
            (Some("ping"), Some(id)) => output.send(&response(&id, json!({}))),
            (Some("tools/list"), Some(id)) => {
                output.send(&response(&id, json!({ "tools": tools::definitions(writer, session.colleague) })))
            }
            (Some("tools/call"), Some(id)) => {
                let (session, output, key) = (session.clone(), output.clone(), id.to_string());
                let handle = calls.spawn(async move {
                    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                    match name {
                        "run" => {
                            let answer = session.run(&arguments, params.get("_meta")).await;
                            output.send(&response(&id, tools::result(&answer.content, answer.is_error)));
                        }
                        name if writer.lists(name) => {
                            let answer = session.write(name, &arguments, params.get("_meta")).await;
                            output.send(&response(&id, tools::result(&answer.content, answer.is_error)));
                        }
                        "ask_colleague" if !session.colleague => {
                            let answer = session.ask(&arguments, params.get("_meta")).await;
                            output.send(&response(&id, tools::result(&answer.content, answer.is_error)));
                        }
                        "message_colleague" if !session.colleague => {
                            let answer = session.message(&arguments, params.get("_meta")).await;
                            output.send(&response(&id, tools::result(&answer.content, answer.is_error)));
                        }
                        _ => output.send(&error_response(&id, -32602, &format!("unknown tool: {name}"))),
                    }
                    id.to_string()
                });
                in_flight.insert(key, handle);
            }
            (Some("notifications/cancelled"), None) => {
                // A cancelled call gets no answer. What it started in the
                // World goes on: a job does not end with the call.
                if let Some(request) = params.get("requestId")
                    && let Some(handle) = in_flight.remove(&request.to_string())
                {
                    handle.abort();
                }
            }
            (Some(_), None) => {}
            (Some(method), Some(id)) => {
                output.send(&error_response(&id, -32601, &format!("method not found: {method}")))
            }
            // Answers to requests this server never sends.
            (None, _) => {}
        }
        while let Some(done) = calls.try_join_next() {
            if let Ok(key) = done {
                in_flight.remove(&key);
            }
        }
    }
    while calls.join_next().await.is_some() {}
    drop(output);
    printing.await??;
    Ok(())
}

fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
