//! A thread item as the World reads it: a message the agent wrote, words
//! that went in, a tool it used, or something not shown, such as its
//! reasoning. Only the fields named here are read; an item of a type not
//! known here is passed on as a tool use named by its type, with nothing
//! said of it (`world.md`, 接入宿主只靠它们写明的接口).

use serde_json::Value;

pub enum Item {
    Message { text: String },
    Words { text: String },
    Tool { tool: String, detail: String, failed: bool },
    Quiet,
}

/// The item's id and what it is.
pub fn read(item: &Value) -> (String, Item) {
    let id = text(item, "id").unwrap_or_default();
    let kind = text(item, "type").unwrap_or_default();
    let failed = matches!(text(item, "status").as_deref(), Some("failed" | "declined"))
        || item.get("error").is_some_and(|error| !error.is_null());
    let tool = |tool: String, detail: String| Item::Tool { tool, detail, failed };
    let read = match kind.as_str() {
        "agentMessage" => Item::Message { text: text(item, "text").unwrap_or_default() },
        "userMessage" => Item::Words { text: user_text(item) },
        "reasoning" | "plan" | "contextCompaction" | "hookPrompt" | "functionCallOutput" | "enteredReviewMode"
        | "exitedReviewMode" | "sleep" | "subAgentActivity" => Item::Quiet,
        "commandExecution" => tool("shell".to_string(), first_line(&text(item, "command").unwrap_or_default())),
        "fileChange" => {
            let paths: Vec<String> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|change| text(change, "path"))
                .collect();
            tool("apply_patch".to_string(), paths.join(", "))
        }
        "mcpToolCall" => {
            let server = text(item, "server").unwrap_or_default();
            let name = text(item, "tool").unwrap_or_default();
            let detail = arguments_detail(&name, item.get("arguments"));
            tool(format!("{server}.{name}"), detail)
        }
        "dynamicToolCall" => {
            let namespace = text(item, "namespace").unwrap_or_default();
            let name = text(item, "tool").unwrap_or_default();
            tool(format!("{namespace}.{name}"), arguments_detail(&name, item.get("arguments")))
        }
        "collabAgentToolCall" => tool(
            format!("collab.{}", text(item, "tool").unwrap_or_default()),
            first_line(&text(item, "prompt").unwrap_or_default()),
        ),
        "webSearch" => tool("web_search".to_string(), text(item, "query").unwrap_or_default()),
        "imageView" => tool("view_image".to_string(), text(item, "path").unwrap_or_default()),
        "imageGeneration" => tool("image_generation".to_string(), String::new()),
        "" => Item::Quiet,
        other => tool(other.to_string(), String::new()),
    };
    (id, read)
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

/// What a tool was used on, in a line: the command a `run` ran and where,
/// the file an edit changed, the files a patch touched; for other tools the
/// first argument that names a thing.
fn arguments_detail(tool: &str, arguments: Option<&Value>) -> String {
    let arguments = match arguments {
        Some(Value::String(json)) => serde_json::from_str(json).unwrap_or(Value::Null),
        Some(arguments) => arguments.clone(),
        None => Value::Null,
    };
    let on = text(&arguments, "node").map(|node| format!(" (on {node})")).unwrap_or_default();
    let detail = match tool {
        "run" => text(&arguments, "command").map(|command| first_line(&command)),
        "apply_patch" => text(&arguments, "input").map(|patch| {
            let files: Vec<&str> = patch
                .lines()
                .filter_map(|line| {
                    ["*** Add File: ", "*** Update File: ", "*** Delete File: "]
                        .iter()
                        .find_map(|head| line.strip_prefix(head))
                })
                .collect();
            files.join(", ")
        }),
        "ask_colleague" => text(&arguments, "description"),
        "message_colleague" => text(&arguments, "to"),
        _ => ["file_path", "path", "query", "url", "command"].iter().find_map(|name| text(&arguments, name)),
    };
    match detail {
        Some(detail) => format!("{}{on}", first_line(&detail)),
        None => String::new(),
    }
}

/// A command's first line, with `…` when more follows.
fn first_line(text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}
