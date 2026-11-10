//! What a device's installed records say of a scope to one of its readers.
//! A reader says a scope ended only from a fact recorded here, under the
//! holding period it was answered in. An answer that brings none of them leaves
//! the scope's state unknown: unreachable, not installed and still arriving
//! are not endings.
use super::{
    catalog::Lifecycle,
    cursor::{HoldingPeriod, VersionRef},
};
use serde::{Deserialize, Serialize};

/// A recorded fact that ends one reader's reading of a scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "end", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeEnd {
    /// The scope's own record is no longer live: its hub's signed description
    /// says so, or the World hosting its subject closed it. Deleted is final; a
    /// removed scope may be described live again at a later ACL revision.
    Lifecycle { lifecycle: Lifecycle },
    /// The object read was deleted at this version, which is final.
    Deleted { version: VersionRef },
    /// At the period's ACL revision this reader has no right to read here. A
    /// later revision may grant it again.
    Refused { reason: String },
}

/// The holding period a reader is answered under and what, by the installed
/// records, ends its reading. Both coordinates of a period only advance, so of
/// two answers about one scope the one with the later (ACL revision,
/// publication epoch) is the current one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeStanding {
    pub period: HoldingPeriod,
    pub end: Option<ScopeEnd>,
}
