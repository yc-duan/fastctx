//! Existing persistence boundaries retained until their registered migration.
//!
//! This inventory is not permission to add another path under a broad prefix.
//! Family and namespace containers require an explicit logical entry as well.

use super::{DataClass, DecisionState, Stage};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Table,
    Family,
    File,
    Browser,
    Container,
    External,
    Migration,
    Reference,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Fixed(DataClass),
    /// A physical container has no single business class; its logical selectors do.
    ByKind(&'static str),
}
#[derive(Clone, Copy, Debug)]
pub struct LegacyEntry {
    pub surface: Surface,
    pub selector: &'static str,
    pub classification: Classification,
    pub owner: &'static str,
    pub stage: Stage,
    pub decision: DecisionState,
    pub note: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/legacy_registry.rs"));
