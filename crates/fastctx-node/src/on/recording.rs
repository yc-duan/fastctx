//! Disk work for an on is awaited without occupying its coordination thread.
use crate::{Node, on_delivery::Following};
use fastctx_platform::doing::MemoryMutex as Mutex;
use fastctx_types::{JobId, ipc::Failure, on::OnOrigin};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) async fn job(node: &Arc<Node>, origin: &OnOrigin, job: &JobId) -> Result<PathBuf, Failure> {
    let (node, origin, job) = (node.clone(), origin.clone(), job.clone());
    fastctx_platform::task::spawn_blocking(move || super::on_job(&node, &origin, &job))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
}

pub(super) async fn taken(waiting: &Arc<Mutex<Following>>, job: &JobId) -> Result<(), Failure> {
    let path = waiting.lock().receipt_path(job);
    let Some(path) = path else { return Ok(()) };
    let waiting = waiting.clone();
    fastctx_platform::task::spawn_blocking(move || {
        crate::records::items::exists(&path)?;
        let (directory, family, key, _) = crate::record_database::item(&path)?;
        crate::records::write_then_at(
            &directory,
            fastctx_storage::Priority::Normal,
            256,
            move |rows| {
                let now = jiff::Timestamp::now();
                rows.put_raw(&family, &key, b"")?;
                rows.expire(&family, &key, now + jiff::SignedDuration::from_hours(30 * 24))?;
                rows.connection().execute(
                    "UPDATE conversation_records SET modified=?4 WHERE conversation=?1 AND family=?2 AND identity=?3",
                    (rows.conversation(), &family, &key, now.to_string()),
                )?;
                Ok(())
            },
            move |_| {
                // Registration outlives cancellation and OutcomeUnknown until the
                // writer resolves the FULL commit, as the receipt reader requires.
                drop(waiting);
            },
        )
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(format!("keeping the on receipt: {error}")))
}

pub(super) async fn piped_taken(node: &Arc<Node>, record_path: &Path) -> Result<(), Failure> {
    let (node, path) = (node.clone(), record_path.to_path_buf());
    fastctx_platform::task::spawn_blocking(move || -> std::io::Result<()> {
        crate::job_metadata::put_raw(&path.with_extension("on-piped"), b"")?;
        if crate::job_metadata::exists(&path.with_extension("on-taken"))? {
            crate::job_watch::copied(&node, &path)?;
        }
        Ok(())
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(format!("recording piped output receipt: {error}")))
}
