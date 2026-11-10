//! Sibling order is separate from item identity and from display parentage.
use crate::sync::{cursor::Sequence, identity::ObjectId};
use serde::{Deserialize, Serialize};

/// Native ordinals preserve the host's own order, including clock regressions.
/// A gap names the next native sibling, or the end of this sibling list. Its
/// members sort by original time, World source sequence and stable input identity.
/// Resolving a missing anchor is durable work; it never guesses another sibling.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum DisplayOrder {
    Native { sequence: Sequence },
    Gap { before: Option<ObjectId>, at: jiff::Timestamp, sequence: Sequence, identity: ObjectId },
}
impl DisplayOrder {
    pub fn anchor(&self) -> Option<&ObjectId> {
        match self {
            Self::Gap { before, .. } => before.as_ref(),
            Self::Native { .. } => None,
        }
    }
}
