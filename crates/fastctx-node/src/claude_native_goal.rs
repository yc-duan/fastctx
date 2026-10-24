//! Wait for the native goal cleanup's transcript output before accepting controls.

use fastctx_hosts::claude::exchange;
use serde_json::{Value, json};
use std::fs::File;
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
            let file = match File::open(path) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if let Some(mut file) = file {
                if after.is_none() {
                    let mut reader = BufReader::new(file);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        let size = reader.read_line(&mut line)?;
                        if size == 0 || !line.ends_with('\n') {
                            return output(json!({"offset":offset}));
                        }
                        offset += size as u64;
                    }
                }
                if file.metadata()?.len() < offset {
                    return Err(io::Error::other("Claude transcript changed during native goal cleanup"));
                }
                file.seek(SeekFrom::Start(offset))?;
                let mut reader = BufReader::new(file);
                let mut line = String::new();
                loop {
                    line.clear();
                    let size = reader.read_line(&mut line)?;
                    if size == 0 || !line.ends_with('\n') {
                        break;
                    }
                    offset += size as u64;
                    let row: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
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
