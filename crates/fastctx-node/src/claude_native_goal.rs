//! Wait for the native goal cleanup's transcript output before accepting controls.

use fastctx_hosts::claude::exchange;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn run(world: &Path, session: &str, after: Option<u64>) -> io::Result<()> {
    crate::claude::pairing::validate_session(session)?;
    let directory = exchange::sessions(world).join(session);
    let began = Instant::now();
    let mut offset = after.unwrap_or(0);
    loop {
        if let Some(path) = exchange::transcript(&directory)? {
            let file = match fastctx_platform::disk::open(path) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if let Some(mut file) = file {
                if after.is_none() {
                    // Where the last whole line ends: found a block at a
                    // time, no line held whole (design §28.49).
                    let mut reader = BufReader::new(file);
                    let mut position = 0u64;
                    loop {
                        let block = reader.fill_buf()?;
                        if block.is_empty() {
                            return output(json!({"offset":offset}));
                        }
                        if let Some(at) = block.iter().rposition(|byte| *byte == b'\n') {
                            offset = position + at as u64 + 1;
                        }
                        let length = block.len();
                        position += length as u64;
                        reader.consume(length);
                    }
                }
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
                    let size = match fastctx_hosts::bounded::read_bounded(&mut reader, &mut line)? {
                        fastctx_hosts::bounded::Read::Line(size) => size,
                        fastctx_hosts::bounded::Read::Refused { ended } => {
                            if !ended && !fastctx_hosts::bounded::skip_line(&mut reader)? {
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
                    if fastctx_hosts::bounded::too_many_values(&line) {
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
        if after.is_none() {
            return output(json!({"offset":0}));
        }
        if began.elapsed() >= Duration::from_secs(120) {
            return Err(io::Error::other("Claude did not persist native goal cleanup within two minutes"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn output(value: Value) -> io::Result<()> {
    serde_json::to_writer(io::stdout(), &value).map_err(io::Error::other)
}
