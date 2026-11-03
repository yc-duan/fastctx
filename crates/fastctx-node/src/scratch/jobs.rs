//! Scratch coordinates travel beside nested on output, never in its byte streams.

use crate::{
    job_metadata::Job,
    records::{self, Source},
};
use fastctx_types::{MachineName, scratch::Location};
use std::{io, path::Path};

fn key(location: &Location) -> io::Result<String> {
    Ok(serde_json::to_string(&(&location.hub, &location.machine))?)
}
pub(crate) fn prepare(record: &Path) -> io::Result<Job> {
    let job = Job::open(record)?;
    job.prepare_file("scratch", record.with_extension("scratch"), |path, push| {
        records::legacy::raw_array::<Location>(path, None, &mut |location, body| {
            push(Source::new(key(&location)?, path.to_path_buf(), body)?.replacing())
        })
    })?;
    Ok(job)
}
pub(crate) fn locations(record: &Path, machine: &MachineName) -> io::Result<Vec<Location>> {
    let room = fastctx_hands::run::supervisor::room(record)?;
    let mut locations = vec![Location {
        hub: None,
        machine: machine.clone(),
        path: Ok(fastctx_hands::run::present::display_path(&room.scratch().to_string_lossy())),
    }];
    locations.extend(prepare(record)?.all::<Location>("scratch")?);
    Ok(locations)
}
pub(crate) fn add(record: &Path, paths: Vec<Location>) -> io::Result<()> {
    let job = prepare(record)?;
    for chunk in paths.chunks(16) {
        let chunk =
            chunk.iter().map(|value| key(value).map(|key| (key, value.clone()))).collect::<io::Result<Vec<_>>>()?;
        job.change("scratch", serde_json::to_vec(&chunk)?.len(), move |rows, family| {
            for (key, location) in chunk {
                rows.put(family, &key, &location)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}
