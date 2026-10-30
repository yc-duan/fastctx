//! Whether a Claude session's record ends with the model's answer: the last
//! of its own exchanges (not a subagent's) is an assistant message that
//! stopped at the end of its turn, with nothing from the user or a tool after
//! it. Read from the end of the file, so a long record costs little.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use jiff::Timestamp;
use serde_json::Value;

/// The model's answer a session's record ends with.
#[derive(Debug, PartialEq, Eq)]
pub struct Answered {
    pub at: Timestamp,
    /// Its words, where it has any.
    pub text: Option<String>,
}

/// How much of a record's end is read: its last exchanges are there.
const TAIL: u64 = 512 * 1024;

pub fn answered(path: &Path) -> io::Result<Option<Answered>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines();
    // A read begun inside a record starts with the end of it.
    if start > 0 {
        lines.next();
    }
    let lines: Vec<&str> = lines.collect();
    for line in lines.iter().rev() {
        // A record still being written is not one yet.
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        if row["isSidechain"] == true || !row.get("agentId").is_none_or(Value::is_null) {
            continue;
        }
        match row["type"].as_str() {
            Some("assistant") => {
                let message = &row["message"];
                if message["stop_reason"] != "end_turn" {
                    return Ok(None);
                }
                let Some(at) = row["timestamp"].as_str().and_then(|at| at.parse::<Timestamp>().ok()) else {
                    return Ok(None);
                };
                let said: Vec<&str> = message["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|block| block["type"] == "text")
                            .filter_map(|block| block["text"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                let text = (!said.is_empty()).then(|| said.join("\n\n"));
                return Ok(Some(Answered { at, text }));
            }
            Some("user") => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}
