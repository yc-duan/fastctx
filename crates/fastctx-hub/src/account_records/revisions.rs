//! Immutable revision summaries are disposable indices beside the original records.

use fastctx_types::{account_records::Record, owned_records::Version};
use std::{io, path::Path};

pub(super) fn preserve(directory: &Path, record: &Record) -> io::Result<Version> {
    let summary = Version { metadata: record.metadata.clone(), history: super::storage::history_head(record)? };
    super::history::immutable(
        &directory.join("summaries").join(format!("{}.json", record.metadata.version)),
        &serde_json::to_vec(&summary).map_err(io::Error::other)?,
    )?;
    Ok(summary)
}

pub(crate) fn list(directory: &Path) -> Result<Vec<Version>, String> {
    let directory = match super::history::location(directory) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let files = match fastctx_platform::disk::read_dir(&directory) {
        Ok(files) => files,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut versions = Vec::new();
    for file in files {
        let path = file.map_err(|error| error.to_string())?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let summary = directory.join("summaries").join(path.file_name().expect("revision file"));
        let version = match fastctx_platform::disk::read(&summary) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Old hubs write no summaries. Build only the missing entries,
                // outside the account gate, including after a downgrade.
                let record = super::storage::read(&path)?;
                preserve(&directory, &record).map_err(|error| error.to_string())?
            }
            Err(error) => return Err(error.to_string()),
        };
        versions.push(version);
    }
    Ok(versions)
}
