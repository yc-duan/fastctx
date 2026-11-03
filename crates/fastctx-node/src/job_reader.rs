//! Readers that need a job's record and output across several IO calls.

use std::path::Path;
use std::sync::Arc;

use crate::Node;

pub(crate) use crate::job_files::Lease;

/// Reserves the files before reading; collection and registration share one
/// in-memory decision, never a lock held over IO.
pub(crate) fn hold(node: &Node, path: &Path) -> Option<Lease> {
    node.job_files.hold(path)
}

/// What reading a job that is gone says.
pub(crate) fn gone() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, "the job is gone")
}

pub(crate) async fn open(node: &Arc<Node>, path: &Path) -> Result<Lease, fastctx_types::ipc::Failure> {
    let _place = match crate::data::places::of_path(node, path) {
        Some(place) => Some(node.places.using(&place).await),
        None => None,
    };
    let (node, path) = (node.clone(), path.to_path_buf());
    fastctx_platform::task::spawn_blocking(move || {
        let lease = hold(&node, &path).ok_or_else(|| fastctx_types::ipc::Failure::new(gone().to_string()))?;
        fastctx_hands::run::record::read(&path).map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?;
        Ok(lease)
    })
    .await
    .map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?
}
