//! Which file events can mean a change, and how they name what changed.
//!
//! Linux reports every opening and closing of a watched file, reads
//! included, and the node reads the very files it watches: a watcher that
//! woke on those would wake itself again with each look. Windows and macOS
//! report no reads, so there a watcher behaves the same either way.

use std::io;
use std::path::{Path, PathBuf};

use notify::EventKind;
use notify::event::{MetadataKind, ModifyKind};

/// The name a watch's events give the existing `path`, to compare their
/// paths against. Linux and Windows name it as it was watched; macOS names
/// the real place behind any link in it, as a Claude session's directory is
/// once it lives in its conversation.
pub(crate) fn named(path: &Path) -> io::Result<PathBuf> {
    if cfg!(target_os = "macos") { std::fs::canonicalize(path) } else { Ok(path.to_path_buf()) }
}

/// Whether `event` can have changed what is in a watched place: anything but
/// a read, or the access time a read moves.
pub(crate) fn changes(event: &notify::Event) -> bool {
    event.need_rescan()
        || !event.kind.is_access()
            && !matches!(event.kind, EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)))
}

/// As `changes`, for a watcher's raw result: an error always counts, since
/// it means a change may have been missed.
pub(crate) fn may_change(event: &notify::Result<notify::Event>) -> bool {
    event.as_ref().map_or(true, changes)
}
