//! Immutable write facts are indexed individually; publication and its stream
//! position are one SQL commit, so no artifact/journal crash window remains.

use super::*;
use crate::{job_metadata::Job, records::Source};
use std::io::BufRead;

pub(crate) fn keep(directory: &Path, fact: &WriteFact) -> io::Result<()> {
    valid(fact)?;
    let record = directory.with_extension("json");
    let job = prepare(&record)?;
    let fact = fact.clone();
    job.change("writes", serde_json::to_vec(&fact)?.len(), move |rows, family| {
        if let Some(previous) = rows.get::<WriteFact>(family, &fact.id)? {
            if previous != fact {
                return Err(fastctx_storage::Error::new("write fact identity changed"));
            }
            return Ok(());
        }
        rows.put(family, &fact.id, &fact)
    })
}

pub(crate) fn prepare(record: &Path) -> io::Result<Job> {
    let job = Job::open(record)?;
    let record = record.to_path_buf();
    job.prepare("writes", move |push| {
        let folder = record.with_extension("writes");
        let journal = record.with_extension("writes.jsonl");
        crate::record_migration::file(&journal, push, |push| {
            let mut reader = io::BufReader::new(fastctx_platform::disk::open(&journal)?);
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 || line.last() != Some(&b'\n') {
                    break;
                }
                let fact: WriteFact = serde_json::from_slice(&line)?;
                valid(&fact)?;
                // The line end is the journal's: the same fact kept as a file
                // of its own has none, and the two are one record.
                line.pop();
                push(Source::new(fact.id.clone(), journal.clone(), line.clone())?)?;
            }
            Ok(())
        })?;
        for path in crate::retention::entries(&folder)? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            crate::record_migration::file(&path, push, |push| {
                let bytes = fastctx_platform::disk::read(&path)?;
                let fact: WriteFact = serde_json::from_slice(&bytes)?;
                valid(&fact)?;
                if path.file_stem().and_then(|name| name.to_str()) != Some(fact.id.as_str()) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "write fact identity differs from its filename",
                    ));
                }
                push(Source::new(fact.id.clone(), path.clone(), bytes)?)
            })?;
        }
        let pending = record.with_extension("writes-pending");
        crate::record_migration::file(&pending, push, |push| {
            let bytes = fastctx_platform::disk::read(&pending)?;
            let fact: WriteFact = serde_json::from_slice(&bytes)?;
            valid(&fact)?;
            push(Source::new(fact.id.clone(), pending.clone(), bytes)?)
        })
    })?;
    Ok(job)
}

pub(super) fn recover(record: &Path, _legacy: bool) -> io::Result<()> {
    prepare(record).map(|_| ())
}

fn valid(fact: &WriteFact) -> io::Result<()> {
    if fact.id.is_empty() || !fact.id.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid write fact identity"));
    }
    Ok(())
}
