//! Whether a Claude session was compacted since the model last answered in
//! it: of the session's own rows (not a subagent's), the last that is either
//! the boundary a compaction leaves or a message of the model is that
//! boundary. A compaction is no turn of Claude Code's: nothing else the
//! session tells says it happened between two turns, by whoever asked for
//! it. Read from the end of the file, as `ending` reads it.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// Which came last in a session's record.
#[derive(Debug, PartialEq, Eq)]
pub enum Last {
    /// A compaction: what the model holds now is what it left.
    Compaction,
    /// A message of the model, written with what it held then.
    Answer,
}

/// How much of a record's end is read: after its boundary a compaction
/// leaves its summary and the files it reads again, a few hundred KiB.
const TAIL: u64 = 4 << 20;

/// Which of a compaction and a message of the model came last in the record
/// at `path`; none when neither is in its last `TAIL` bytes.
pub fn last(path: &Path) -> io::Result<Option<Last>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(TAIL).read_to_end(&mut bytes)?;
    Ok(last_in(&bytes, start > 0))
}

/// `last` of the end of a record, `cut` where it begins inside a row.
fn last_in(bytes: &[u8], cut: bool) -> Option<Last> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    if cut {
        lines.next();
    }
    for line in lines.rev() {
        // Only these rows are looked into; a row still being written is
        // not one yet.
        if !line.contains("\"assistant\"") && !line.contains("compact_boundary") {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        if row["isSidechain"] == true || !row.get("agentId").is_none_or(Value::is_null) {
            continue;
        }
        match (row["type"].as_str(), row["subtype"].as_str()) {
            (Some("system"), Some("compact_boundary")) => return Some(Last::Compaction),
            // What Claude Code writes in the model's place when a request
            // fails is no answer of the model's.
            (Some("assistant"), _) if row["isApiErrorMessage"] != true && row["message"]["model"] != "<synthetic>" => {
                return Some(Last::Answer);
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(rows: &[Value]) -> Vec<u8> {
        rows.iter().map(|row| format!("{row}\n")).collect::<String>().into_bytes()
    }

    fn answer() -> Value {
        json!({"type":"assistant","uuid":"a","message":{"model":"claude-opus-5-5","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}})
    }

    fn boundary(trigger: &str) -> Value {
        json!({"type":"system","subtype":"compact_boundary","uuid":"b","compactMetadata":{"trigger":trigger,"preTokens":830_281}})
    }

    fn summary() -> Value {
        json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":"This session is being continued from a previous conversation; the assistant said so."}})
    }

    fn rest() -> Vec<Value> {
        vec![
            json!({"type":"system","subtype":"turn_duration","durationMs":1200}),
            json!({"type":"last-prompt"}),
            json!({"type":"ai-title","aiTitle":"x"}),
        ]
    }

    #[test]
    fn a_compaction_after_the_last_answer_came_last() {
        let mut rows = vec![answer()];
        rows.extend(rest());
        rows.extend([boundary("manual"), summary(), json!({"type":"attachment","attachment":{"type":"file"}})]);
        rows.extend(rest());
        assert_eq!(last_in(&record(&rows), false), Some(Last::Compaction));
        // Two in a row, as when one more followed the user's.
        rows.extend([boundary("manual"), summary()]);
        assert_eq!(last_in(&record(&rows), false), Some(Last::Compaction));
    }

    /// One the host made inside a turn, or the user's before more turns, is
    /// past: the model answered since.
    #[test]
    fn an_answer_after_a_compaction_came_last() {
        let mut rows = vec![answer(), boundary("auto"), summary(), json!({"type":"user","message":{"content":"go on"}}), answer()];
        rows.extend(rest());
        assert_eq!(last_in(&record(&rows), false), Some(Last::Answer));
        assert_eq!(last_in(&record(&[answer()]), false), Some(Last::Answer));
    }

    #[test]
    fn a_subagent_or_an_error_in_the_models_place_is_no_answer() {
        let rows = vec![
            answer(),
            boundary("manual"),
            summary(),
            json!({"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5-5"}}),
            json!({"type":"assistant","agentId":"a1","message":{"model":"claude-haiku-5-5"}}),
            json!({"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>"}}),
            json!({"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"No response requested."}]}}),
        ];
        assert_eq!(last_in(&record(&rows), false), Some(Last::Compaction));
        // A subagent's compaction is not the session's.
        let rows = vec![answer(), json!({"type":"system","subtype":"compact_boundary","isSidechain":true})];
        assert_eq!(last_in(&record(&rows), false), Some(Last::Answer));
    }

    #[test]
    fn a_row_cut_short_at_either_end_is_passed_over() {
        let mut bytes = br#"ype":"system","subtype":"compact_boundary"}"#.to_vec();
        bytes.push(b'\n');
        bytes.extend(record(&[answer()]));
        bytes.extend(br#"{"type":"system","subtype":"compact_boun"#);
        assert_eq!(last_in(&bytes, true), Some(Last::Answer));
        assert_eq!(last_in(&record(&rest()), false), None);
    }

    #[test]
    fn the_end_of_a_file_is_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let mut rows = vec![answer()];
        // More than is read before what decides it: the start is cut.
        let filler = json!({"type":"user","message":{"content":"x".repeat(1 << 20)}});
        rows.extend(std::iter::repeat_n(filler, 5));
        rows.extend([answer(), boundary("manual"), summary()]);
        std::fs::write(&path, record(&rows)).unwrap();
        assert_eq!(last(&path).unwrap(), Some(Last::Compaction));
    }
}
