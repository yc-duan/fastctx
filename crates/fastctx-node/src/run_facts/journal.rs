//! Executor detail uses bounded segments and retains every segment until deletion.

use super::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SEGMENT: u64 = 8 * 1024 * 1024;

#[derive(Default, Serialize, Deserialize)]
struct Index {
    first: u64,
    current: u64,
}

fn index(record: &Path) -> io::Result<Index> {
    match fastctx_platform::disk::read(record.with_extension("steps-index")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Index::default()),
        Err(error) => Err(error),
    }
}

fn path(record: &Path, segment: u64) -> PathBuf {
    if segment == 0 {
        record.with_extension("steps.jsonl")
    } else {
        record.with_extension("steps").join(format!("{segment:016x}.jsonl"))
    }
}

fn save(record: &Path, index: &Index) -> io::Result<()> {
    publish(&record.with_extension("steps-index"), &serde_json::to_vec(index)?, Permissions::Private)
}

pub(super) fn append(record: &Path, row: &[u8]) -> io::Result<()> {
    if row.len() as u64 > SEGMENT {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "one step exceeds the journal's 8 MiB segment budget"));
    }
    let mut index = index(record)?;
    let length = match fastctx_platform::disk::metadata(path(record, index.current)) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    if length > 0 && length.saturating_add(row.len() as u64) > SEGMENT {
        let next = index.current.checked_add(1).ok_or_else(|| io::Error::other("step journal segment exhausted"))?;
        fastctx_platform::ensure_private_dir(&record.with_extension("steps"))?;
        let next_path = path(record, next);
        match publish_new(&next_path, &[], Permissions::Private) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        index.current = next;
        save(record, &index)?;
    }
    log::append(&path(record, index.current), row)
}

pub(super) fn read_since(
    record: &Path,
    segment: &mut u64,
    offset: &mut u64,
) -> io::Result<Vec<fastctx_types::run_facts::RunStep>> {
    let index = index(record)?;
    if *segment < index.first {
        return Err(io::Error::other("the step cursor is older than the retained journal"));
    }
    loop {
        let file = path(record, *segment);
        if !fastctx_platform::disk::exists(&file)? && (*offset > 0 || index.current > 0) {
            return Err(io::Error::other("a retained step journal segment is missing"));
        }
        log::repair(&file)?;
        let rows = super::cursor::json(&file, offset)?;
        if !rows.is_empty() || *segment >= index.current {
            return Ok(rows);
        }
        *segment += 1;
        *offset = 0;
    }
}

/// The first retained segment is the only recoverable starting point when
/// upgrading an executor whose earlier detail has already expired.
pub(super) fn first(record: &Path) -> io::Result<u64> {
    Ok(index(record)?.first)
}
