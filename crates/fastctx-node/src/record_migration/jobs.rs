//! A malformed supervisor record cannot supply a trustworthy job incarnation.
//! Keep its sidecars in place and quarantine only the invalid metadata file.
use fastctx_platform::{disk, readings};
use std::{io, path::Path};

pub(super) fn open(node: &crate::Node, path: &Path) -> io::Result<Option<crate::job_metadata::Job>> {
    let bytes = match disk::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            readings::counter("migration.source_read_retries").add(1);
            return Err(error);
        }
    };
    let record = match serde_json::from_slice::<fastctx_types::run::JobRecord>(&bytes) {
        Ok(record) => record,
        Err(error) => {
            let (directory, _, _, _) = crate::record_database::item(path)?;
            let (_, scope) = crate::record_database::registered(&directory)?;
            super::archive::damaged(
                node.world.path(),
                &scope,
                "job-supervisor",
                path,
                &format!(
                    "{error}; the job incarnation cannot be recovered; associated sidecars remain in place and have not been imported"
                ),
            )?;
            return Ok(None);
        }
    };
    crate::job_metadata::Job::of(path, &record).map(Some)
}
