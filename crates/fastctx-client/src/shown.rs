//! What every interface says the same way of a session's items: what a
//! tool was used on, in a line. Words around it are each interface's own.

use fastctx_types::session::{ChangeHow, Does, Tool};

/// What a tool was used on, in a line: the command it ran, the files it
/// changed and by how much, what it searched for, the colleague it asked;
/// for any other, the first argument that names a thing.
pub fn tool_detail(tool: &Tool) -> String {
    match &tool.does {
        Does::Command { command, .. } => first_line(command),
        Does::Edit { changes } => {
            let files: Vec<String> = changes
                .iter()
                .map(|change| {
                    let path = match &change.how {
                        ChangeHow::Moved { to } => format!("{} → {}", short_path(&change.path), short_path(to)),
                        _ => short_path(&change.path),
                    };
                    let added = change.diff.lines().filter(|line| line.starts_with('+')).count();
                    let removed = change.diff.lines().filter(|line| line.starts_with('-')).count();
                    match (added, removed, &change.how) {
                        (_, _, ChangeHow::Deleted) => format!("{path} −"),
                        (0, 0, _) => path,
                        (added, 0, _) => format!("{path} +{added}"),
                        (added, removed, _) => format!("{path} +{added} −{removed}"),
                    }
                })
                .collect();
            files.join(", ")
        }
        Does::Search { query } => query.clone(),
        Does::Image { path } => path.as_deref().map(short_path).unwrap_or_default(),
        Does::Colleague { to, task } => match (to, task) {
            (Some(to), Some(task)) => format!("{to} · {task}"),
            (Some(one), None) | (None, Some(one)) => one.clone(),
            (None, None) => String::new(),
        },
        Does::Agent { kind, task, agents, .. } => match (kind, task) {
            (Some(kind), Some(task)) => format!("{kind} · {}", first_line(task)),
            (Some(one), None) => one.clone(),
            (None, Some(task)) => first_line(task),
            (None, None) => agents.join(", "),
        },
        Does::Call => ["file_path", "path", "query", "url", "command", "description", "pattern", "code", "prompt"]
            .iter()
            .find_map(|key| tool.input.get(key).and_then(|value| value.as_str()))
            .map(first_line)
            .unwrap_or_default(),
    }
}

/// A path's last two parts: enough to know the file in a row.
pub fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|part| !part.is_empty()).collect();
    match parts.len() {
        0..=2 => path.to_string(),
        n => format!("…/{}/{}", parts[n - 2], parts[n - 1]),
    }
}

/// The first line of a text, with `…` when more follows.
fn first_line(text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}
