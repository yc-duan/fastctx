//! Segmented executor detail: 8 MiB per segment, at most 32 retained segments.
//! Only segments behind the durable emission cursor may expire after seven days
//! or be evicted for the size bound. Unsent detail applies backpressure instead.

use super::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const SEGMENT: u64 = 8 * 1024 * 1024;
const SEGMENTS: u64 = 32;
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Default, Serialize, Deserialize)]
struct Index {
    first: u64,
    current: u64,
}

fn index(record: &Path) -> io::Result<Index> {
    match std::fs::read(record.with_extension("steps-index")) {
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

pub(super) fn append(record: &Path, row: &[u8], sent: u64) -> io::Result<()> {
    let mut index = index(record)?;
    let length = match std::fs::metadata(path(record, index.current)) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    if length > 0 && length.saturating_add(row.len() as u64) > SEGMENT {
        let next = index.current.checked_add(1).ok_or_else(|| io::Error::other("step journal segment exhausted"))?;
        prune(record, &mut index, Some(next), sent)?;
        if next - index.first >= SEGMENTS {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "the unsent step journal reached its 256 MiB budget",
            ));
        }
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

fn prune(record: &Path, index: &mut Index, next: Option<u64>, sent: u64) -> io::Result<()> {
    let old = index.first;
    while index.first < index.current && index.first < sent {
        let crowded = next.is_some_and(|next| next - index.first >= SEGMENTS);
        let expired = match std::fs::metadata(path(record, index.first)) {
            Ok(metadata) => SystemTime::now().duration_since(metadata.modified()?).is_ok_and(|age| age >= KEEP),
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => return Err(error),
        };
        if !crowded && !expired {
            break;
        }
        index.first += 1;
    }
    if index.first != old {
        save(record, index)?;
        for segment in old..index.first {
            match std::fs::remove_file(path(record, segment)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    if index.first > 0 {
        match std::fs::remove_file(path(record, 0)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let directory = record.with_extension("steps");
    if directory.try_exists()? {
        for entry in std::fs::read_dir(directory)? {
            let file = entry?.path();
            let number =
                file.file_stem().and_then(|name| name.to_str()).and_then(|name| u64::from_str_radix(name, 16).ok());
            if number.is_some_and(|number| number < index.first) {
                std::fs::remove_file(file)?;
            }
        }
    }
    Ok(())
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
        if !file.try_exists()? && (*offset > 0 || index.current > 0) {
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

pub(super) fn read(record: &Path, mut apply: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    let index = index(record)?;
    for segment in index.first..=index.current {
        log::read(&path(record, segment), &mut apply)?;
    }
    Ok(())
}
