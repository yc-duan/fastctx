//! What the plugin's native goal cleanup needs from the transcript: before it,
//! whether the transcript holds a goal Claude Code could restore; after it,
//! the cleanup's own rows, before controls are accepted.

use fastctx_hosts::bounded::{self, Read};
use fastctx_hosts::claude::exchange;
use serde_json::{Value, json};
use std::io::{self, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

/// The attachment Claude Code restores a native goal from when it resumes a
/// session: the conversation's last one, unless that one was met or failed
/// (2.1.322). Quoted, it is a string of the record itself; inside the text of
/// a message its quotes are escaped.
const GOAL_STATUS: &str = "\"goal_status\"";

pub fn run(world: &Path, session: &str, after: Option<u64>) -> io::Result<()> {
    crate::claude::pairing::validate_session(session)?;
    let directory = exchange::sessions(world).join(session);
    let Some(mut offset) = after else {
        return output(before(&directory)?);
    };
    let began = Instant::now();
    loop {
        if let Some(path) = exchange::transcript(&directory)? {
            let file = match fastctx_platform::disk::open(path) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if let Some(mut file) = file {
                if file.metadata()?.len() < offset {
                    return Err(io::Error::other("Claude transcript changed during native goal cleanup"));
                }
                file.seek(SeekFrom::Start(offset))?;
                let mut reader = BufReader::new(file);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    // A line larger than World reads is not the cleanup's:
                    // gone over unread, once whole (design §28.49).
                    let size = match bounded::read_bounded(&mut reader, &mut line)? {
                        Read::Line(size) => size,
                        Read::Refused { ended } => {
                            if !ended && !bounded::skip_line(&mut reader)? {
                                break;
                            }
                            offset = reader.stream_position()?;
                            continue;
                        }
                    };
                    if size == 0 || line.last() != Some(&b'\n') {
                        break;
                    }
                    offset += size as u64;
                    if bounded::too_many_values(&line) {
                        continue;
                    }
                    let row: Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
                    if row["type"] == "system"
                        && row["subtype"] == "local_command"
                        && row["isSidechain"] != true
                        && row.pointer("/commandRun/command").and_then(Value::as_str) == Some("goal")
                        && row.pointer("/commandRun/args").and_then(Value::as_str) == Some("clear")
                    {
                        if row.get("commandOutcome").is_some_and(|outcome| !outcome.is_null()) {
                            return Err(io::Error::other("Claude native goal cleanup did not complete"));
                        }
                        return output(json!({"offset":offset,"uuid":row["uuid"]}));
                    }
                }
            }
        }
        if began.elapsed() >= Duration::from_secs(120) {
            return Err(io::Error::other("Claude did not persist native goal cleanup within two minutes"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Where the transcript's last whole line ends, and whether it holds a goal
/// record (`goal`): told when one was found, or when the whole transcript was
/// read and none was. Left out, the plugin clears as though one were there.
fn before(directory: &Path) -> io::Result<Value> {
    let Some(path) = exchange::transcript(directory)? else {
        return Ok(json!({"offset":0}));
    };
    let file = match fastctx_platform::disk::open(path) {
        Ok(file) => file,
        // Not written yet, which says nothing of a goal in the history the
        // session starts from.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(json!({"offset":0})),
        Err(error) => return Err(error),
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let (mut offset, mut goal, mut whole) = (0u64, false, true);
    loop {
        line.clear();
        let size = match bounded::read_bounded(&mut reader, &mut line)? {
            Read::Line(size) => size,
            // Far longer than a goal record: gone over unread (design §28.49).
            Read::Refused { ended } => {
                if !ended && !bounded::skip_line(&mut reader)? {
                    whole = false;
                    break;
                }
                offset = reader.stream_position()?;
                continue;
            }
        };
        if size == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            // Still being written.
            whole = false;
            break;
        }
        offset += size as u64;
        if !goal && mentions(&line, GOAL_STATUS) {
            match goal_record(&line) {
                Some(found) => goal = found,
                None => whole = false,
            }
        }
    }
    Ok(if goal || whole { json!({"offset":offset,"goal":goal}) } else { json!({"offset":offset}) })
}

/// Whether a line naming `goal_status` is a goal record: `None` where it
/// cannot be read.
fn goal_record(line: &[u8]) -> Option<bool> {
    if bounded::too_many_values(line) {
        return None;
    }
    let row: Value = serde_json::from_slice(line).ok()?;
    Some(row["type"] == "attachment" && row["attachment"]["type"] == "goal_status")
}

fn mentions(line: &[u8], word: &str) -> bool {
    match std::str::from_utf8(line) {
        Ok(text) => text.contains(word),
        Err(_) => line.windows(word.len()).any(|window| window == word.as_bytes()),
    }
}

fn output(value: Value) -> io::Result<()> {
    serde_json::to_writer(io::stdout(), &value).map_err(io::Error::other)
}
