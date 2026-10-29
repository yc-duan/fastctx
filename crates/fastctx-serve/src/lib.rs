//! `fastctx serve`: the MCP server an agent session starts.
//!
//! It speaks MCP's stdio transport with the host and turns each tool call
//! into a call to this machine's node, starting the node if it is not
//! running. It keeps nothing of the World itself: when the node restarts,
//! calls in flight fail and say that whether they took effect is unknown;
//! the session goes on and the next call reaches the new node.

mod answer;
mod protocol;
mod run_failure;
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

/// The MCP revisions this server speaks; it answers in the client's when it
/// knows it, and in the newest otherwise, as the protocol asks.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Serves one session until its host closes standard input, then lets the
/// calls in flight answer. Each call ends within the `run` wait, so this
/// ends too.
pub async fn serve(world: WorldDir, writes_off: bool) -> anyhow::Result<()> {
    serve_starting(world, writes_off, None).await
}

pub async fn serve_starting(
    world: WorldDir,
    writes_off: bool,
    startup: Option<fastctx_platform::FileLock>,
) -> anyhow::Result<()> {
    let host = fastctx_platform::host_process::parent_identity();
    let gone = async {
        match host {
            Ok(host) => match fastctx_platform::host_process::exited(&host).await {
                Ok(()) => return,
                Err(error) => eprintln!(
                    "fastctx serve: observing its host failed: {error}; standard input still determines its lifetime"
                ),
            },
            Err(error) => eprintln!(
                "fastctx serve: identifying its host failed: {error}; standard input still determines its lifetime"
            ),
        }
        std::future::pending::<()>().await;
    };
    tokio::select! {
        result = serve_session(world, writes_off, startup) => result,
        () = gone => {
            eprintln!("fastctx serve: the process that started it exited; leaving");
            Ok(())
        }
    }
}

async fn serve_session(
    world: WorldDir,
    writes_off: bool,
    mut startup: Option<fastctx_platform::FileLock>,
) -> anyhow::Result<()> {
    let session = Arc::new(Session::new(world, writes_off)?);
    let mut input = stdio::input();
    let (output, printing) = Output::start();
    let mut calls = JoinSet::new();
    let mut in_flight: HashMap<String, AbortHandle> = HashMap::new();
    let mut ended = "standard input ended";
    loop {
        let input = tokio::select! {
            input = input.recv() => match input { Some(input) => input, None => break },
            _ = calls.join_next(), if !calls.is_empty() => {
                in_flight.retain(|_, handle| !handle.is_finished());
                if in_flight.is_empty() { in_flight = HashMap::new(); }
                if calls.is_empty() { calls = JoinSet::new(); }
                continue;
            },
        };
        let line = match input {
            Input::Line(line) => line,
            Input::Ended => break,
            Input::Failed(error) => {
                eprintln!("fastctx serve: reading standard input: {error}");
                ended = "reading standard input failed";
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
        let protocol = match (method, id.as_ref()) {
            (Some(method), Some(id)) => match protocol::Protocol::request(method, &params, session.initialized()) {
                Ok(protocol) => protocol,
                Err(error) => {
                    output.send(&json!({ "jsonrpc": "2.0", "id": id, "error": error }));
                    continue;
                }
            },
            _ => protocol::Protocol::Legacy,
        };
        match (method, id) {
            (Some("initialize"), Some(id)) => {
                startup.take();
                session.introduce(params.get("clientInfo").unwrap_or(&Value::Null));
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked.filter(|asked| PROTOCOL_VERSIONS.contains(asked)).unwrap_or(PROTOCOL_VERSIONS[0]);
                output.send(&response(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": { "listChanged": true } },
                        "serverInfo": { "name": "fastctx", "version": env!("CARGO_PKG_VERSION") }
                    }),
                ));
            }
            (Some("server/discover"), Some(id)) => {
                startup.take();
                output.send(&protocol.response(&id, protocol::discovery(), true));
            }
            (Some("ping"), Some(id)) => output.send(&protocol.response(&id, json!({}), false)),
            (Some("tools/list"), Some(id)) => {
                let meta = params.get("_meta");
                let machines = match session.machines(meta).await {
                    Ok(machines) => machines,
                    Err(error) => {
                        output.send(&error_response(&id, -32603, &error));
                        continue;
                    }
                };
                let tools = tools::definitions(writes_off, session.woken(), session.codex(meta), machines.as_ref());
                output.send(&protocol.response(&id, json!({ "tools": tools }), true));
                session.watch_tools(&output, meta);
            }
            (Some("tools/call"), Some(id)) => {
                let main = session.clone();
                let (session, output, key) = (session.for_call(params.get("_meta")), output.clone(), id.to_string());
                let handle = calls.spawn(async move {
                    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                    let mut answer = match name {
                        "run" => session.run(&arguments, params.get("_meta")).await,
                        // Sessions whose host listed the tools before writing
                        // became the shell's commands may still call these.
                        "edit" | "write" | "apply_patch" => session.write(name, &arguments, params.get("_meta")).await,
                        // Whether this conversation may ask for colleagues is
                        // the node's to say: a host may still hold the tools
                        // after the user turned them off.
                        "ask_colleague" => session.ask(&arguments, params.get("_meta")).await,
                        "message_colleague" => session.message(&arguments, params.get("_meta")).await,
                        // The node says again whether this session speaks.
                        fastctx_types::voice::TOOL => session.say(&arguments, params.get("_meta")).await,
                        _ => {
                            output.send(&error_response(&id, -32602, "unknown tool"));
                            return id.to_string();
                        }
                    };
                    // A call that reached a node starts watching it, for
                    // the resident it now knows.
                    main.watch_tools(&output, params.get("_meta"));
                    match session.bound(&mut answer) {
                        Ok(()) => {
                            if answer.facts.is_some() {
                                let kept = match session.client(params.get("_meta")).await {
                                    Ok(client) => answer.keep_facts(client).await.map_err(|error| error.to_string()),
                                    Err(error) => Err(error),
                                };
                                if let Err(error) = kept {
                                    output.send(&error_response(
                                        &id,
                                        -32603,
                                        &format!("could not preserve call facts: {error}"),
                                    ));
                                    return id.to_string();
                                }
                            }
                            let response =
                                protocol.response(&id, tools::result(&answer.content, answer.is_error), false);
                            let _ = output.send_answer(&response, answer).await;
                        }
                        Err(error) => {
                            eprintln!("fastctx serve: could not preserve the full answer: {error}");
                            output.send(&error_response(
                                &id,
                                -32603,
                                "could not preserve the full answer; see the server's diagnostics",
                            ));
                        }
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
            // Codex offers its models resource tools for every server; this
            // one has none, and files are read in the shell.
            (Some(method), Some(id)) if method.starts_with("resources/") => output.send(&error_response(
                &id,
                -32601,
                "fastctx has no resources; to read a file, call the run tool with `fastctx view PATH`",
            )),
            (Some(method), Some(id)) => {
                output.send(&error_response(&id, -32601, &format!("method not found: {method}")))
            }
            // Answers to requests this server never sends.
            (None, _) => {}
        }
        while calls.try_join_next().is_some() {}
        in_flight.retain(|_, handle| !handle.is_finished());
        if in_flight.is_empty() {
            in_flight = HashMap::new();
        }
        if calls.is_empty() {
            calls = JoinSet::new();
        }
    }
    eprintln!("fastctx serve: {ended}; answering the calls in flight, then leaving");
    while calls.join_next().await.is_some() {}
    session.stop_watching();
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
