//! Executor visits are a durable set, with a monotonically indexed append stream.
use crate::{job_metadata::Job, records::Source};
use fastctx_types::{JobId, MachineName};
use std::{
    io::{self, BufRead},
    path::Path,
};

type Visit = (MachineName, JobId);
fn parse(bytes: &[u8]) -> io::Result<Visit> {
    let text = std::str::from_utf8(bytes).map_err(io::Error::other)?.trim_end();
    let (machine, job) = text.split_once(' ').ok_or_else(|| io::Error::other("invalid executor visit"))?;
    Ok((machine.try_into().map_err(io::Error::other)?, job.try_into().map_err(io::Error::other)?))
}
fn key(visit: &Visit) -> String {
    format!("{} {}", visit.0, visit.1)
}
fn prepare(record: &Path) -> io::Result<Job> {
    let job = Job::open(record)?;
    job.prepare("visits", |push| {
        let path = record.with_extension("on");
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for line in io::BufReader::new(file).split(b'\n') {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            let visit = parse(&line)?;
            push(Source::new(key(&visit), path.clone(), serde_json::to_vec(&visit)?)?)?;
        }
        Ok(())
    })?;
    Ok(job)
}
pub(super) fn keep(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let visit = parse(bytes)?;
    let job = prepare(&path.with_extension("json"))?;
    job.change("visits", bytes.len(), move |rows, family| rows.put(family, &key(&visit), &visit))
}
pub(super) fn page(record: &Path, cursor: &mut u64) -> io::Result<Vec<Visit>> {
    prepare(record)?.page("visits", cursor)
}
pub(super) fn all(record: &Path) -> io::Result<Vec<Visit>> {
    prepare(record)?.all("visits")
}
