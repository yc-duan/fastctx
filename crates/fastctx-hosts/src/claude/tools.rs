//! A Claude Code tool use as the World shows it. Claude Code names an MCP
//! tool `mcp__<server>__<tool>`, a plugin's server `plugin_<plugin>_<server>`;
//! its own tools by their names (`Bash`, `Edit`). The World's own tools read
//! as they do from any host (`world_tools`); Claude Code's own that do the
//! same things as Codex's show the same way.

use fastctx_types::session::{AgentAct, Change, ChangeHow, Content, Does, Tool, ToolStatus};
use serde_json::Value;

use crate::world_tools;

pub fn tool(full_name: &str, input: Value, output: Vec<Content>, status: ToolStatus, duration_ms: Option<u64>) -> Tool {
    let (server, name) = match full_name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
        Some((server, name)) => (Some(server.to_string()), name.to_string()),
        None => (None, full_name.to_string()),
    };
    let world = server
        .as_deref()
        .is_some_and(|server| server == world_tools::SERVER || server.ends_with(&format!("_{}", world_tools::SERVER)));
    let (does, machine) = if world {
        world_tools::read(&name, &input, &output)
    } else if server.is_none() {
        (own(&name, &input), None)
    } else {
        (Does::Call, None)
    };
    Tool { run: None, name, server, machine, does, input, output, status, duration_ms }
}

/// Claude Code's own tools that do what a shell, an edit or a search does.
fn own(name: &str, input: &Value) -> Does {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
    let mut diff = String::new();
    match name {
        "Bash" => Does::Command { command: text("command").unwrap_or_default(), cwd: None, exit: None, job: None },
        "Edit" => {
            lines(&mut diff, '-', &text("old_string").unwrap_or_default());
            lines(&mut diff, '+', &text("new_string").unwrap_or_default());
            Does::Edit {
                changes: vec![Change { path: text("file_path").unwrap_or_default(), how: ChangeHow::Updated, diff }],
            }
        }
        "MultiEdit" => {
            for edit in input.get("edits").and_then(Value::as_array).into_iter().flatten() {
                let part = |key: &str| edit.get(key).and_then(Value::as_str).unwrap_or_default();
                diff.push_str("@@\n");
                lines(&mut diff, '-', part("old_string"));
                lines(&mut diff, '+', part("new_string"));
            }
            Does::Edit {
                changes: vec![Change { path: text("file_path").unwrap_or_default(), how: ChangeHow::Updated, diff }],
            }
        }
        "Write" => {
            lines(&mut diff, '+', &text("content").unwrap_or_default());
            Does::Edit {
                changes: vec![Change { path: text("file_path").unwrap_or_default(), how: ChangeHow::Replaced, diff }],
            }
        }
        "WebSearch" => Does::Search { query: text("query").unwrap_or_default() },
        // Which agent it started is in its answer, or told as it starts.
        "Agent" | "Task" => Does::Agent {
            act: AgentAct::Start,
            agents: Vec::new(),
            kind: text("subagent_type"),
            task: text("description"),
        },
        // To an agent by its id or its name, as Claude Code's ListAgents has it.
        "SendMessage" => Does::Agent {
            act: AgentAct::Message,
            agents: text("to").into_iter().collect(),
            kind: None,
            task: text("message").or_else(|| text("text")),
        },
        _ => Does::Call,
    }
}

fn lines(diff: &mut String, mark: char, text: &str) {
    for line in text.lines() {
        diff.push(mark);
        diff.push_str(line);
        diff.push('\n');
    }
}
