//! A thread item as the World reads it (Codex 0.160's `ThreadItem`).
//! Words that went in are told apart, as whoever sent them tells them; every
//! other item is something the agent did, shown as the official app shows
//! it: a reasoning item by its summary, a command with its output, a file
//! change with its diff. An item of a type not known here is passed on by
//! its type, with nothing said of it (`world.md`, 接入宿主只靠它们写明的接口).

use fastctx_types::session::{
    AgentAct, Change, ChangeHow, Content, Does, HostAgentEnd, HostAgentStart, Item, SessionChange, Tool, ToolStatus,
    TurnHow, UserMessage, What,
};
use serde_json::Value;

use crate::world_tools;

pub enum Read {
    /// Words that went into the thread, their text parts joined.
    Words {
        text: String,
        message: UserMessage,
    },
    Item(Item),
}

pub fn read(item: &Value, completed: bool) -> Read {
    let id = text(item, "id").unwrap_or_default();
    let kind = text(item, "type").unwrap_or_default();
    let mut what = match kind.as_str() {
        "userMessage" => {
            let Some((text, presentation)) = super::presentation::user_message(item) else { return unshown(item) };
            return Read::Words {
                text,
                message: UserMessage {
                    attachments: Vec::new(),
                    images: Vec::new(),
                    id,
                    client_id: item.get("clientId").and_then(Value::as_str).map(str::to_string),
                    content: array(item, "content"),
                    hook_fragments: Vec::new(),
                    presentation,
                    detail: item.clone(),
                },
            };
        }
        "hookPrompt" => {
            let fragments = array(item, "fragments");
            let text = fragments.iter().filter_map(|f| f.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(
                "
",
            );
            if text.trim().is_empty() {
                return unshown(item);
            }
            return Read::Words {
                text,
                message: UserMessage {
                    attachments: Vec::new(),
                    images: Vec::new(),
                    id,
                    client_id: None,
                    content: Vec::new(),
                    hook_fragments: fragments,
                    presentation: Value::Null,
                    detail: item.clone(),
                },
            };
        }
        "functionCallOutput" => {
            let Some(message) = super::presentation::function_message(item) else { return unshown(item) };
            let mut projected = read(&message, completed);
            if let Read::Words { message, .. } = &mut projected {
                message.detail = item.clone();
            }
            return projected;
        }
        "sleep" | "enteredReviewMode" | "exitedReviewMode" => return unshown(item),
        "subAgentActivity" => What::AgentActivity {
            agent: text(item, "agentThreadId").unwrap_or_default(),
            path: text(item, "agentPath").unwrap_or_default(),
            activity: text(item, "kind").unwrap_or_default(),
        },
        "agentMessage" => What::Message { text: text(item, "text").unwrap_or_default() },
        // A reasoning item is shown by its summary; one without a summary is
        // still thinking that happened, as Claude's kept words are.
        "reasoning" => What::Thinking { text: strings(item, "summary").join("\n\n"), duration_ms: None },
        "plan" => What::Plan { text: text(item, "text").unwrap_or_default() },
        "contextCompaction" => What::Compaction,
        "commandExecution" => {
            let command = text(item, "command").unwrap_or_default();
            let cwd = text(item, "cwd");
            let output = text(item, "aggregatedOutput").map(|text| Content::Text { text }).into_iter().collect();
            tool(
                item,
                "shell",
                None,
                Does::Command {
                    command: command.clone(),
                    cwd: cwd.clone(),
                    exit: item.get("exitCode").and_then(Value::as_i64),
                    job: None,
                },
            )
            .input(serde_json::json!({ "command": command, "cwd": cwd }))
            .output(output)
            .done()
        }
        "fileChange" => {
            if item.get("changes").and_then(Value::as_array).is_some_and(Vec::is_empty) {
                return unshown(item);
            }
            let changes = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|change| Change {
                    path: text(change, "path").unwrap_or_default(),
                    how: match change.pointer("/kind/type").and_then(Value::as_str) {
                        Some("add") => ChangeHow::Added,
                        Some("delete") => ChangeHow::Deleted,
                        _ => match change.pointer("/kind/move_path").and_then(Value::as_str) {
                            Some(to) => ChangeHow::Moved { to: to.to_string() },
                            None => ChangeHow::Updated,
                        },
                    },
                    diff: text(change, "diff").unwrap_or_default(),
                })
                .collect();
            tool(item, "apply_patch", None, Does::Edit { changes })
                .input(item.get("changes").cloned().unwrap_or(Value::Null))
                .done()
        }
        "mcpToolCall" => {
            let server = text(item, "server").unwrap_or_default();
            if server == "codex_app" {
                let mut dynamic = item.clone();
                dynamic["type"] = serde_json::json!("dynamicToolCall");
                dynamic["namespace"] = serde_json::json!("codex_app");
                dynamic["success"] = if item["status"] == "inProgress" {
                    Value::Null
                } else {
                    serde_json::json!(item["status"] == "completed" && item["error"].is_null())
                };
                dynamic["contentItems"] = match item.pointer("/result/content").and_then(Value::as_array) {
                    None => Value::Null,
                    Some(blocks) => blocks
                        .iter()
                        .filter(|block| block["type"] == "text")
                        .filter_map(|block| {
                            Some(serde_json::json!({"type":"inputText", "text":block["text"].as_str()?}))
                        })
                        .collect(),
                };
                let mut projected = read(&dynamic, completed);
                if let Read::Item(projected) = &mut projected {
                    // The descriptor and original result remain available for
                    // the official non-approved review's native MCP view.
                    projected.detail = Some(item.clone());
                }
                return projected;
            }
            let name = text(item, "tool").unwrap_or_default();
            let input = arguments(item.get("arguments"));
            let output = item
                .pointer("/result/content")
                .and_then(Value::as_array)
                .map(|blocks| world_tools::mcp_content(blocks))
                .unwrap_or_default();
            let (does, machine) = match server.as_str() {
                world_tools::SERVER => world_tools::read(&name, &input, &output),
                _ => (Does::Call, None),
            };
            let mut used = tool(item, &name, Some(server), does).input(input).output(output);
            used.tool.machine = machine;
            used.done()
        }
        "dynamicToolCall" => {
            if super::presentation::hidden_tool(item) {
                return unshown(item);
            }
            let output = item
                .get("contentItems")
                .and_then(Value::as_array)
                .map(|blocks| dynamic_content(blocks))
                .unwrap_or_default();
            let mut used = tool(item, &text(item, "tool").unwrap_or_default(), text(item, "namespace"), Does::Call)
                .input(arguments(item.get("arguments")))
                .output(output);
            if item.get("success").and_then(Value::as_bool) == Some(false) {
                used.tool.status = ToolStatus::Failed;
            }
            used.done()
        }
        // Codex's own agents (`multi_agent`): each is a thread of its own,
        // named here by its id (Codex 0.162's `CollabAgentTool`).
        "collabAgentToolCall" => {
            let name = text(item, "tool").unwrap_or_default();
            let act = match name.as_str() {
                "spawnAgent" => AgentAct::Start,
                "resumeAgent" => AgentAct::Resume,
                "wait" => AgentAct::Wait,
                "closeAgent" | "interruptAgent" => AgentAct::Stop,
                "listAgents" => AgentAct::List,
                _ => AgentAct::Message,
            };
            let mut used = tool(
                item,
                &name,
                None,
                Does::Agent { act, agents: strings(item, "receiverThreadIds"), kind: None, task: text(item, "prompt") },
            )
            .input(serde_json::json!({ "prompt": item.get("prompt"), "model": item.get("model") }));
            if text(item, "status").as_deref() == Some("interrupted") {
                used.tool.status = ToolStatus::Cut;
            }
            used.done()
        }
        "webSearch" => tool(item, "web_search", None, Does::Search { query: text(item, "query").unwrap_or_default() })
            .input(item.get("action").cloned().unwrap_or(Value::Null))
            .done(),
        "imageView" => tool(item, "view_image", None, Does::Image { path: text(item, "path") }).done(),
        "imageGeneration" => {
            let source = ["savedPath", "result"]
                .iter()
                .any(|key| item.get(key).and_then(Value::as_str).is_some_and(|value| !value.trim().is_empty()));
            let running = matches!(item.get("status").and_then(Value::as_str), Some("inProgress" | "in_progress"));
            let limit = item.pointer("/failure/type").and_then(Value::as_str) == Some("usageLimitExceeded")
                && item.pointer("/failure/limitId").and_then(Value::as_str) == Some("image_gen");
            if !source && !running && !limit {
                return unshown(item);
            }
            let mut output = Vec::new();
            if let Some(result) = text(item, "result").filter(|result| !result.trim().is_empty()) {
                let image = result.trim();
                let lower = image.to_ascii_lowercase();
                let uri = ["data:image/", "http://", "https://", "file://", "app://"]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix));
                let drive = image.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                    && image.as_bytes().get(1) == Some(&b':')
                    && image.as_bytes().get(2).is_some_and(|byte| matches!(byte, b'/' | b'\\'));
                output.push(if uri || image.starts_with('/') || image.starts_with(r"\\") || drive {
                    image_content(image)
                } else {
                    Content::Image { mime: "image/png".into(), base64: image.into() }
                });
            }
            let mut used =
                tool(item, "image_generation", None, Does::Image { path: text(item, "savedPath") }).output(output);
            if item.get("failure").is_some_and(|failure| !failure.is_null()) {
                used.tool.status = ToolStatus::Failed;
            }
            used.done()
        }
        other => What::Other { host_kind: other.to_string() },
    };
    if let What::Tool(tool) = &mut what
        && !completed
        && item.get("status").is_none()
    {
        tool.status = ToolStatus::Running;
    }
    Read::Item(Item { draft: None, id, what, detail: Some(item.clone()), started_at: None })
}

/// What a finished item tells of Codex's own agents: which call started
/// which, and how one's run ended where the call says. A wait's answer
/// names each agent it waited for and how that one stands; a spawn's names
/// the agent it started (`multi_agent`, Codex 0.162); `subAgentActivity`
/// does both (`multi_agent_v2`).
pub fn agent_facts(item: &Value) -> Vec<SessionChange> {
    let mut facts = Vec::new();
    let id = text(item, "id").unwrap_or_default();
    let started = |agent: String, task: Option<String>, prompt: Option<String>, model: Option<String>| {
        SessionChange::AgentStarted {
            agent: Box::new(HostAgentStart {
                id: agent,
                call: Some(id.clone()),
                task,
                prompt,
                model: model.filter(|model| !model.is_empty()),
                background: true,
                ..Default::default()
            }),
        }
    };
    let ended = |agent: String, how: TurnHow, words: Option<String>| {
        let failed = matches!(how, TurnHow::Failed);
        SessionChange::AgentEnded {
            agent_id: agent,
            end: HostAgentEnd {
                reason: if failed { words.clone() } else { None },
                answer: if failed { None } else { words },
                how,
                duration_ms: None,
                tokens: None,
                tools: None,
                task: None,
                told: false,
            },
        }
    };
    match text(item, "type").as_deref() {
        Some("collabAgentToolCall") => {
            let tool = text(item, "tool").unwrap_or_default();
            if tool == "spawnAgent" {
                for agent in strings(item, "receiverThreadIds") {
                    let prompt = text(item, "prompt");
                    let task = prompt.as_deref().map(first_line);
                    facts.push(started(agent, task, prompt, text(item, "model")));
                }
            }
            if tool == "wait" {
                let states = item.get("agentsStates").and_then(Value::as_object);
                for (agent, state) in states.into_iter().flatten() {
                    let words = text(state, "message").filter(|words| !words.is_empty());
                    let how = match text(state, "status").as_deref() {
                        Some("completed") => TurnHow::Completed,
                        Some("errored") => TurnHow::Failed,
                        Some("interrupted" | "shutdown") => TurnHow::Interrupted,
                        _ => continue,
                    };
                    facts.push(ended(agent.clone(), how, words));
                }
            }
        }
        Some("subAgentActivity") => {
            let agent = text(item, "agentThreadId").unwrap_or_default();
            if !agent.is_empty() {
                match text(item, "kind").as_deref() {
                    // Its path names it under the agents that spawned it,
                    // `/root/<task>`: the task is its last part.
                    Some("started") => {
                        facts.push(started(agent, text(item, "agentPath").map(|path| task_of(&path)), None, None))
                    }
                    Some("completed") => facts.push(ended(agent, TurnHow::Completed, None)),
                    Some("interrupted") => facts.push(ended(agent, TurnHow::Interrupted, None)),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    facts
}

/// The task an agent's path names, `/root/<spawner>/<task>`: its last part.
fn task_of(path: &str) -> String {
    path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(path).to_string()
}

/// The first line of a prompt, as the few words of a task.
fn first_line(prompt: &str) -> String {
    prompt.trim().lines().next().unwrap_or_default().trim().to_string()
}

/// A tool item being read.
struct Used {
    tool: Tool,
}

impl Used {
    fn input(mut self, input: Value) -> Used {
        self.tool.input = input;
        self
    }

    fn output(mut self, output: Vec<Content>) -> Used {
        self.tool.output = output;
        self
    }

    fn done(self) -> What {
        What::Tool(Box::new(self.tool))
    }
}

fn tool(item: &Value, name: &str, server: Option<String>, does: Does) -> Used {
    let status = match text(item, "status").as_deref() {
        Some("inProgress" | "in_progress") => ToolStatus::Running,
        Some("failed") => ToolStatus::Failed,
        Some("declined") => ToolStatus::Declined,
        Some("completed") | None => ToolStatus::Done,
        _ => ToolStatus::Unknown,
    };
    Used {
        tool: Tool {
            run: None,
            name: name.to_string(),
            server,
            machine: None,
            does,
            input: Value::Null,
            output: Vec::new(),
            status,
            duration_ms: item.get("durationMs").and_then(Value::as_u64),
        },
    }
}

/// Arguments are already JSON values on the app-server wire.
fn arguments(arguments: Option<&Value>) -> Value {
    arguments.cloned().unwrap_or(Value::Null)
}

/// A dynamic tool's output items: `inputText` and `inputImage`.
fn dynamic_content(blocks: &[Value]) -> Vec<Content> {
    blocks
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("inputText") => Content::Text { text: text(block, "text").unwrap_or_default() },
            Some("inputImage") => image_content(&text(block, "imageUrl").unwrap_or_default()),
            Some("inputAudio") => Content::AudioUrl { url: text(block, "audioUrl").unwrap_or_default() },
            _ => Content::Other { block: block.clone() },
        })
        .collect()
}

fn image_content(url: &str) -> Content {
    match data_url(url) {
        Some((mime, base64)) => Content::Image { mime, base64 },
        None => Content::ImageUrl { url: url.to_string() },
    }
}

/// `data:<mime>;base64,<data>`.
fn data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    Some((mime.to_string(), data.to_string()))
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn array(value: &Value, key: &str) -> Vec<Value> {
    value.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Hidden items still have identities that an approval review can reference.
fn unshown(item: &Value) -> Read {
    Read::Item(Item {
        draft: None,
        id: text(item, "id").unwrap_or_default(),
        started_at: None,
        detail: Some(item.clone()),
        what: What::Hidden { host_kind: text(item, "type").unwrap_or_default() },
    })
}
