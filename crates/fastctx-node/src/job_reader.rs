//! Readers that need a job's record and output across several IO calls.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Node;

pub(crate) struct Lease {
    node: Arc<Node>,
    path: PathBuf,
}

/// The caller holds the disk gate while resolving and registering a reader;
/// a job being let go of has none, gone as it will be a moment later
/// (`crate::collecting`).
pub(crate) fn hold(node: &Arc<Node>, path: &Path) -> Option<Lease> {
    if crate::collecting::gone(node, path) {
        return None;
    }
    *node.job_readers.lock().expect("never poisoned").entry(path.to_path_buf()).or_default() += 1;
    Some(Lease { node: node.clone(), path: path.to_path_buf() })
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
        let lease = {
            let _gate = node.disk.lock();
            hold(&node, &path)
        }
        .ok_or_else(|| fastctx_types::ipc::Failure::new(gone().to_string()))?;
        fastctx_hands::run::record::read(&path).map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?;
        Ok(lease)
    })
    .await
    .map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut readers = self.node.job_readers.lock().expect("never poisoned");
        if let Some(count) = readers.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                readers.remove(&self.path);
                if self.node.disk_due.lock().expect("never poisoned").contains(&self.path) {
                    self.node.disk_changed.notify_one();
                }
            }
        }
    }
}
