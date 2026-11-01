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

pub(super) fn prepare(record: &Path) -> io::Result<Job> {
    let job = Job::open(record)?;
    job.prepare("writes", |push| {
        let folder = record.with_extension("writes");
        let journal = record.with_extension("writes.jsonl");
        match std::fs::File::open(&journal) {
            Ok(file) => {
                let mut reader = io::BufReader::new(file);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    if reader.read_until(b'\n', &mut line)? == 0 || line.last() != Some(&b'\n') {
                        break;
                    }
                    let fact: WriteFact = serde_json::from_slice(&line)?;
                    valid(&fact)?;
                    let artifact = folder.join(format!("{}.json", fact.id));
                    let source = if artifact.try_exists()? { artifact } else { journal.clone() };
                    push(Source::new(fact.id.clone(), source, serde_json::to_vec(&fact)?)?)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        match std::fs::read_dir(&folder) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    if path.extension().is_none_or(|ext| ext != "json") {
                        continue;
                    }
                    let fact: WriteFact = serde_json::from_slice(&std::fs::read(&path)?)?;
                    valid(&fact)?;
                    push(Source::new(fact.id.clone(), path, serde_json::to_vec(&fact)?)?)?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let pending = record.with_extension("writes-pending");
        match std::fs::read(&pending) {
            Ok(bytes) => {
                let fact: WriteFact = serde_json::from_slice(&bytes)?;
                valid(&fact)?;
                let artifact = folder.join(format!("{}.json", fact.id));
                let source = if artifact.try_exists()? { artifact } else { pending };
                push(Source::new(fact.id.clone(), source, serde_json::to_vec(&fact)?)?)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
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
