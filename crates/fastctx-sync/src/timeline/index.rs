//! Exact previous rows accompany index effects for the executor's transaction.
use super::{TimelineChange, TimelineItem};
use crate::{cursor::Revision, store::Error};
#[derive(Clone, Debug)]
pub enum IndexChange {
    Put { previous: Option<Box<TimelineItem>>, item: Box<TimelineItem> },
    Retract { previous: Box<TimelineItem>, revision: Revision },
}
/// Applies one canonical change, not an untrusted client mutation. Reducer state retains
/// revisions for invisible items and rejects stale puts before this entry.
/// The returned previous row is the writer's compare-and-swap precondition.
pub fn index_changes(previous: Option<&TimelineItem>, change: &TimelineChange) -> Result<Option<IndexChange>, Error> {
    match change {
        TimelineChange::Put { item } => {
            if let Some(old) = previous {
                if old.id != item.id || old.revision > item.revision {
                    return Err(Error::Invalid("timeline index revision goes backwards or changes identity".into()));
                }
                if old.revision == item.revision {
                    return if old == item {
                        Ok(None)
                    } else {
                        Err(Error::Invalid("same timeline revision has different content".into()))
                    };
                }
            }
            Ok(Some(IndexChange::Put { previous: previous.cloned().map(Box::new), item: Box::new(item.clone()) }))
        }
        TimelineChange::Retract { item, revision } => {
            let Some(old) = previous else { return Ok(None) };
            if &old.id != item || *revision <= old.revision {
                return Err(Error::Invalid("retraction does not advance this item revision".into()));
            }
            Ok(Some(IndexChange::Retract { previous: Box::new(old.clone()), revision: *revision }))
        }
        TimelineChange::SelectBranch { .. } => {
            Err(Error::Invalid("branch selection rebuilds the index in executor batches".into()))
        }
    }
}
