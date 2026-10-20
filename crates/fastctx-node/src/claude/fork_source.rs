//! Give the native CLI a retained prefix without editing the source transcript.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use fastctx_hosts::claude::{exchange, transcript};
use fastctx_types::ipc::Failure;
use serde_json::Value;

use crate::Node;

/// The temporary native JSONL is kept until the child persists its own history.
pub(super) fn prefix(
    node: &Node,
    session: &str,
    directory: &Path,
    point: &Value,
    checkpoints: &mut Option<Value>,
) -> Result<PathBuf, Failure> {
    let anchor =
        point["retained_until"].as_str().ok_or_else(|| Failure::new("the selected turn has no saved anchor"))?;
    let count = point["hashes"].as_array().ok_or_else(|| Failure::new("the selected turn has no saved prefix"))?.len();
    let original = exchange::transcript(&exchange::sessions(node.world.path()).join(session))
        .map_err(failure)?
        .ok_or_else(|| Failure::new("Claude did not identify its source history"))?;
    let input = std::fs::File::open(original).map_err(failure)?;
    let length = input.metadata().map_err(failure)?.len();
    let source = directory.join("claude-fork-source");
    fastctx_platform::ensure_private_dir(&source).map_err(failure)?;
    let path = source.join(format!("{session}.jsonl"));
    let mut output = std::fs::OpenOptions::new().create_new(true).write(true).open(&path).map_err(failure)?;
    let mut input = std::io::BufReader::new(input.take(length));
    let (mut row, mut offset, mut boundary) = (Vec::new(), 0_u64, None);
    while input.read_until(b'\n', &mut row).map_err(failure)? != 0 {
        output.write_all(&row).map_err(failure)?;
        offset += row.len() as u64;
        if let Ok(value) = serde_json::from_slice::<Value>(&row)
            && value["uuid"] == anchor
            && value["isSidechain"] != true
        {
            boundary = Some(offset);
        }
        row.clear();
    }
    let boundary =
        boundary.ok_or_else(|| Failure::new("the selected anchor is no longer in Claude's saved history"))?;
    output.set_len(boundary).map_err(failure)?;
    output.sync_all().map_err(failure)?;
    drop(output);
    let read = transcript::read(&path).map_err(failure)?;
    if !read.problems.is_empty()
        || read.context_uuids.len() != count
        || read.context_uuids.last().map(String::as_str) != Some(anchor)
    {
        return Err(Failure::new("the saved prefix does not match the selected turn; its native history changed"));
    }
    if let Some(checkpoints) = checkpoints {
        checkpoints["messages"] = point["hashes"].clone();
        checkpoints["uuids"] =
            serde_json::to_value(&read.context_uuids).map_err(|error| Failure::new(error.to_string()))?;
        checkpoints["last_uuid"] = anchor.into();
        checkpoints["last_raw_uuid"] =
            serde_json::to_value(read.context_tail).map_err(|error| Failure::new(error.to_string()))?;
        if let Some(turns) = checkpoints["turns"].as_object_mut() {
            turns.retain(|_, point| point["after"].as_u64().is_some_and(|after| after <= count as u64));
        }
    }
    Ok(path)
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}
