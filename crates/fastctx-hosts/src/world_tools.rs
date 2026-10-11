//! The World's own tools as a session's events show them, whichever host
//! called them: a `run` is a command on a machine, `edit`, `write` and
//! `apply_patch` change files, the colleague tools ask or tell a colleague.
//! The World knows these tools' arguments and the first line of their
//! answers (`world.md`, 位置诚实), so it reads them as what they do.

use fastctx_types::MachineName;
use fastctx_types::session::{Change, ChangeHow, Content, Does};
use serde_json::Value;

/// The name the World's tools are served under in every host.
pub const SERVER: &str = "fastctx";

/// What one of the World's tools does, read from its arguments and, once
/// it has answered, its answer; with the machine it acted on.
pub fn read(tool: &str, input: &Value, output: &[Content]) -> (Does, Option<MachineName>) {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
    let head = output.iter().find_map(|piece| match piece {
        Content::Text { text } => text.lines().next(),
        Content::Other { block: value } if value["type"] == "text" => {
            value["text"].as_str().and_then(|text| text.lines().next())
        }
        _ => None,
    });
    let machine = if tool == "run" {
        None
    } else {
        text("node").and_then(|node| node.parse().ok()).or_else(|| head.and_then(machine_of))
    };
    let does = match tool {
        "run" => {
            Does::Command { command: text("command").unwrap_or_default(), cwd: text("cwd"), exit: None, job: None }
        }
        "apply_patch" => Does::Edit { changes: patch_changes(&text("input").unwrap_or_default()) },
        "edit" => {
            let removed = text("old_string").unwrap_or_default();
            let added = text("new_string").unwrap_or_default();
            let mut diff = String::new();
            push_lines(&mut diff, '-', &removed);
            push_lines(&mut diff, '+', &added);
            Does::Edit {
                changes: vec![Change { path: text("file_path").unwrap_or_default(), how: ChangeHow::Updated, diff }],
            }
        }
        "write" => {
            let mut diff = String::new();
            push_lines(&mut diff, '+', &text("content").unwrap_or_default());
            Does::Edit {
                changes: vec![Change { path: text("file_path").unwrap_or_default(), how: ChangeHow::Replaced, diff }],
            }
        }
        // The answer's head names the colleague it started.
        "ask_colleague" => Does::Colleague {
            to: head.and_then(|head| head.strip_prefix("=== colleague ")?.split(' ').next().map(str::to_string)),
            task: text("description"),
        },
        "message_colleague" => Does::Colleague { to: text("to"), task: None },
        _ => Does::Call,
    };
    (does, machine)
}

/// The machine an answer's head note names: `=== <what> on <machine> (`.
fn machine_of(head: &str) -> Option<MachineName> {
    let rest = head.strip_prefix("=== ")?;
    let (_, after) = rest.split_once(" on ")?;
    let (machine, _) = after.split_once(' ')?;
    machine.parse().ok()
}

/// The files a patch in the World's `apply_patch` grammar changes, each
/// with its hunks as they are written.
pub fn patch_changes(patch: &str) -> Vec<Change> {
    let mut changes: Vec<Change> = Vec::new();
    for line in patch.lines() {
        let headed = [
            ("*** Add File: ", ChangeHow::Added),
            ("*** Delete File: ", ChangeHow::Deleted),
            ("*** Update File: ", ChangeHow::Updated),
        ]
        .into_iter()
        .find_map(|(head, how)| line.strip_prefix(head).map(|path| (path, how)));
        if let Some((path, how)) = headed {
            changes.push(Change { path: path.trim().to_string(), how, diff: String::new() });
            continue;
        }
        let Some(change) = changes.last_mut() else { continue };
        if let Some(to) = line.strip_prefix("*** Move to: ") {
            change.how = ChangeHow::Moved { to: to.trim().to_string() };
        } else if line.starts_with("*** End") {
            continue;
        } else if line.starts_with(['+', '-', ' ']) || line.starts_with("@@") || line.is_empty() {
            change.diff.push_str(line);
            change.diff.push('\n');
        }
    }
    changes
}

fn push_lines(diff: &mut String, mark: char, text: &str) {
    for line in text.lines() {
        diff.push(mark);
        diff.push_str(line);
        diff.push('\n');
    }
}

/// An MCP tool result's content blocks: text and images, in order. A block
/// of another type is said as what it is.
pub fn mcp_content(blocks: &[Value]) -> Vec<Content> {
    blocks
        .iter()
        .map(|block| {
            let text = |key: &str| block.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
            match block.get("type").and_then(Value::as_str) {
                Some("text") if block.as_object().is_some_and(|o| o.len() == 2) => Content::Text { text: text("text") },
                Some("image") if block.as_object().is_some_and(|o| o.len() == 3) => {
                    Content::Image { mime: text("mimeType"), base64: text("data") }
                }
                _ => Content::Other { block: block.clone() },
            }
        })
        .collect()
}
