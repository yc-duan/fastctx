//! Jobs' files let go of off the jobs' gate (`ranked::Disk`): a job's files,
//! a scratch or output directory, a conversation's places. The gate is held
//! only to decide that nothing reads them and to mark them; the deleting,
//! as long as a slow disk takes, is done after it is let go. Under the gate
//! a path being let go of is gone already, as it will be a moment later: no
//! reader takes it and no receipt is written in it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Node;

/// What one collection marked, let go of when dropped.
pub(crate) struct Collecting {
    node: Arc<Node>,
    paths: Vec<PathBuf>,
}

/// Why paths could not be marked.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Held {
    /// A reader holds one of them, or a job within one.
    Read,
    /// Another collection has one of them, or a path within or around one.
    Collecting,
}

/// Marks `paths` as being let go of. Asked under the jobs' gate, so that no
/// reader comes between the look and the mark.
pub(crate) fn begin(node: &Arc<Node>, paths: Vec<PathBuf>) -> Result<Collecting, Held> {
    let overlaps = |path: &Path| paths.iter().any(|ours| path.starts_with(ours) || ours.starts_with(path));
    if node.job_readers.lock().expect("never poisoned").keys().any(|path| overlaps(path)) {
        return Err(Held::Read);
    }
    let mut collecting = node.collecting.lock().expect("never poisoned");
    if collecting.iter().any(|path| overlaps(path)) {
        return Err(Held::Collecting);
    }
    collecting.extend(paths.iter().cloned());
    drop(collecting);
    Ok(Collecting { node: node.clone(), paths })
}

/// Whether `path` is being let go of, or lies within what is. Asked under
/// the jobs' gate, where it is gone.
pub(crate) fn gone(node: &Node, path: &Path) -> bool {
    node.collecting.lock().expect("never poisoned").iter().any(|collected| path.starts_with(collected))
}

impl Drop for Collecting {
    fn drop(&mut self) {
        let mut collecting = self.node.collecting.lock().expect("never poisoned");
        for path in &self.paths {
            if let Some(at) = collecting.iter().position(|collected| collected == path) {
                collecting.swap_remove(at);
            }
        }
    }
}
