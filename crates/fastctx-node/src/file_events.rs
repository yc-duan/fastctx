//! Which file events can mean a change.
//!
//! Linux reports every opening and closing of a watched file, reads
//! included, and the node reads the very files it watches: a watcher that
//! woke on those would wake itself again with each look. Windows and macOS
//! report no reads, so there a watcher behaves the same either way.

use notify::EventKind;
use notify::event::{MetadataKind, ModifyKind};

/// Whether `event` can have changed what is in a watched place: anything but
/// a read, or the access time a read moves.
pub(crate) fn changes(event: &notify::Event) -> bool {
    !event.kind.is_access() && !matches!(event.kind, EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)))
}

/// As `changes`, for a watcher's raw result: an error always counts, since
/// it means a change may have been missed.
pub(crate) fn may_change(event: &notify::Result<notify::Event>) -> bool {
    event.as_ref().map_or(true, changes)
}
