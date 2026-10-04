//! A thread item as the World reads it (Codex 0.160's `ThreadItem`, the
//! inventory in `dev-notes/research_notes/驱动官方智能体/2026-10-04-Codex-0.160协议清单/items.md`).
//! Words that went in are told apart, as whoever sent them tells them; every
//! other item is something the agent did, shown as the official app shows
//! it: a reasoning item by its summary, a command with its output, a file
//! change with its diff. An item of a type not known here is passed on by
//! its type, with nothing said of it (`world.md`, 接入宿主只靠它们写明的接口).

use fastctx_types::session::{Change, ChangeHow, Content, Does, Item, Tool, ToolStatus, What};
use serde_json::Value;

use crate::world_tools;

pub enum Read {
    /// Words that went into the thread, their text parts joined.
    Words(String),
    Item(Item),
}

pub fn read(item: &Value) -> Read {
    let id = text(item, "id").unwrap_or_default();
    let kind = text(item, "type").unwrap_or_default();
    let what = match kind.as_str() {
        "userMessage" => return Read::Words(user_text(item)),
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
            let name = text(item, "tool").unwrap_or_default();
            let input = arguments(item.get("arguments"));
            let mut output = item
                .pointer("/result/content")
                .and_then(Value::as_array)
                .map(|blocks| world_tools::mcp_content(blocks))
                .unwrap_or_default();
            if let Some(message) = item.pointer("/error/message").and_then(Value::as_str) {
                output.push(Content::Text { text: message.to_string() });
            }
            let (does, machine) = match server.as_str() {
                world_tools::SERVER => world_tools::read(&name, &input, &output),
                _ => (Does::Call, None),
            };
            let mut used = tool(item, &name, Some(server), does).input(input).output(output);
            used.tool.machine = machine;
            used.done()
        }
        "dynamicToolCall" => {
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
            if let Some(prompt) = text(item, "revisedPrompt") {
                output.push(Content::Text { text: prompt });
            }
            if let Some(image) = text(item, "result").filter(|result| !result.is_empty()) {
                output.push(Content::Image { mime: "image/png".to_string(), base64: image });
            }
            let mut used =
                tool(item, "image_generation", None, Does::Image { path: text(item, "savedPath") }).output(output);
            if item.get("failure").is_some_and(|failure| !failure.is_null()) {
                used.tool.status = ToolStatus::Failed;
            }
            used.done()
        }
        "sleep" => tool(item, "sleep", None, Does::Call)
            .input(serde_json::json!({ "durationMs": item.get("durationMs") }))
            .done(),
        other => What::Other { host_kind: other.to_string() },
    };
    Read::Item(Item { id, what })
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
        _ => ToolStatus::Done,
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

/// Arguments as the host gave them: an object, or JSON text of one.
fn arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(json)) => serde_json::from_str(json).unwrap_or_else(|_| Value::String(json.clone())),
        Some(arguments) => arguments.clone(),
        None => Value::Null,
    }
}

/// A dynamic tool's output items: `inputText` and `inputImage`.
fn dynamic_content(blocks: &[Value]) -> Vec<Content> {
    blocks
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("inputText") => Content::Text { text: text(block, "text").unwrap_or_default() },
            Some("inputImage") => match text(block, "imageUrl").and_then(|url| data_url(&url)) {
                Some((mime, base64)) => Content::Image { mime, base64 },
                None => Content::Text { text: text(block, "imageUrl").unwrap_or_default() },
            },
            other => Content::Text { text: format!("[{} content]", other.unwrap_or("unknown")) },
        })
        .collect()
}

/// `data:<mime>;base64,<data>`.
fn data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    Some((mime.to_string(), data.to_string()))
}

/// The text parts of words that went in, joined.
fn user_text(item: &Value) -> String {
    let parts: Vec<String> = item
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| text(part, "text"))
        .collect();
    parts.join("\n")
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
