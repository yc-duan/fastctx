//! A thread item as the World reads it (Codex 0.160's `ThreadItem`, the
//! inventory in `dev-notes/research_notes/驱动官方智能体/2026-10-04-Codex-0.160协议清单/items.md`).
//! Words that went in are told apart, as whoever sent them tells them; every
//! other item is something the agent did, shown as the official app shows
//! it: a reasoning item by its summary, a command with its output, a file
//! change with its diff. An item of a type not known here is passed on by
//! its type, with nothing said of it (`world.md`, 接入宿主只靠它们写明的接口).

use fastctx_types::session::{
    Change, ChangeHow, Content, Does, Item, ItemDetails, Tool, ToolStatus, UserMessage, What,
};
use serde_json::Value;

use crate::world_tools;

pub enum Read {
    /// Words that went into the thread, their text parts joined.
    Words {
        text: String,
        message: UserMessage,
    },
    Hidden,
    Item(Item),
}

pub fn read(item: &Value, completed: bool) -> Read {
    let id = text(item, "id").unwrap_or_default();
    let kind = text(item, "type").unwrap_or_default();
    let mut what = match kind.as_str() {
        "userMessage" => {
            let Some((text, presentation)) = super::presentation::user_message(item) else { return Read::Hidden };
            return Read::Words {
                text,
                message: UserMessage {
                    id,
                    client_id: item.get("clientId").and_then(Value::as_str).map(str::to_string),
                    content: array(item, "content"),
                    hook_fragments: Vec::new(),
                    presentation,
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
                return Read::Hidden;
            }
            return Read::Words {
                text,
                message: UserMessage {
                    id,
                    client_id: None,
                    content: Vec::new(),
                    hook_fragments: fragments,
                    presentation: Value::Null,
                },
            };
        }
        "functionCallOutput" => {
            return super::presentation::function_message(item)
                .map(|item| read(&item, completed))
                .unwrap_or(Read::Hidden);
        }
        "sleep" | "enteredReviewMode" | "exitedReviewMode" => return Read::Hidden,
        "subAgentActivity" => What::AgentActivity {
            agent: text(item, "agentThreadId").unwrap_or_default(),
            path: text(item, "agentPath").unwrap_or_default(),
            activity: text(item, "kind").unwrap_or_default(),
        },
        "agentMessage" => What::Message { text: text(item, "text").unwrap_or_default() },
        // The official app shows a reasoning item by its summary only.
        "reasoning" => What::Thinking { text: strings(item, "summary").join("\n\n") },
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
                dynamic["success"] = serde_json::json!(item["status"] == "completed" && item["error"].is_null());
                if super::presentation::hidden_tool(&dynamic) {
                    return Read::Hidden;
                }
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
                return Read::Hidden;
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
        "collabAgentToolCall" => {
            if text(item, "tool").as_deref() == Some("wait") {
                return Read::Hidden;
            }
            let to = item.pointer("/receiverThreadIds/0").and_then(Value::as_str).map(str::to_string);
            tool(
                item,
                &text(item, "tool").unwrap_or_default(),
                None,
                Does::Colleague { to, task: text(item, "prompt") },
            )
            .input(serde_json::json!({ "prompt": item.get("prompt"), "model": item.get("model") }))
            .done()
        }
        "webSearch" => tool(item, "web_search", None, Does::Search { query: text(item, "query").unwrap_or_default() })
            .input(item.get("action").cloned().unwrap_or(Value::Null))
            .done(),
        "imageView" => tool(item, "view_image", None, Does::Image { path: text(item, "path") }).done(),
        "imageGeneration" => {
            let mut output = Vec::new();
            if let Some(image) = text(item, "result").filter(|result| !result.is_empty()) {
                output.push(
                    if data_url(&image).is_some()
                        || image.starts_with("http:")
                        || image.starts_with("https:")
                        || image.starts_with("file:")
                        || image.starts_with("app:")
                        || image.starts_with('/')
                        || image.starts_with("\\\\")
                        || image.as_bytes().get(1) == Some(&b':')
                    {
                        image_content(&image)
                    } else {
                        Content::Image { mime: "image/png".into(), base64: image }
                    },
                );
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
        && !completed && item.get("status").is_none() {
            tool.status = ToolStatus::Running;
        }
    Read::Item(Item { id, what, details: Box::new(details(item)), started_at: None, completed_at: None })
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
            _ => Content::Structured { value: block.clone() },
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

fn details(item: &Value) -> ItemDetails {
    let value = |key: &str| item.get(key).cloned().unwrap_or(Value::Null);
    let nested = |key: &str| item.pointer(key).cloned().unwrap_or(Value::Null);
    match item.get("type").and_then(Value::as_str) {
        Some("agentMessage") => ItemDetails::Message {
            phase: text(item, "phase"),
            delivery: text(item, "delivery"),
            memory_citation: value("memoryCitation"),
            questions: value("questions"),
        },
        Some("commandExecution") => ItemDetails::Command {
            actions: array(item, "commandActions"),
            process_id: text(item, "processId"),
            source: text(item, "source"),
            plugin_id: text(item, "pluginId"),
            script_path: text(item, "scriptPath"),
        },
        Some("mcpToolCall") => ItemDetails::Mcp {
            structured_content: nested("/result/structuredContent"),
            meta: nested("/result/_meta"),
            error: value("error"),
            app_ui: value("mcpAppUi"),
            app_resource_uri: text(item, "mcpAppResourceUri"),
            app_context: value("appContext"),
            plugin_id: text(item, "pluginId"),
            read_only: item.get("readOnlyHint").and_then(Value::as_bool),
        },
        Some("dynamicToolCall") => ItemDetails::Dynamic { success: item.get("success").and_then(Value::as_bool) },
        Some("collabAgentToolCall") => ItemDetails::Colleagues {
            sender: text(item, "senderThreadId").unwrap_or_default(),
            receivers: strings(item, "receiverThreadIds"),
            states: value("agentsStates"),
            model: text(item, "model"),
            reasoning_effort: text(item, "reasoningEffort"),
        },
        Some("webSearch") => ItemDetails::Search { results: value("results") },
        Some("imageGeneration") => ItemDetails::Image {
            status: text(item, "status").unwrap_or_default(),
            revised_prompt: text(item, "revisedPrompt"),
            transparent_background: item.get("transparentBackground").and_then(Value::as_bool),
            failure: value("failure"),
        },
        _ => ItemDetails::None,
    }
}
