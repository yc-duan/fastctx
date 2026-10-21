//! Towns on this machine (`world-projects.md`, `world-journals.md`,
//! `world-map.md`): each town's record and journals, as this machine's copy
//! of its ledger holds them, and what the user and residents do with them.

pub mod caller;
pub mod guard;
pub(crate) mod instructions;
pub mod journal;
pub(crate) mod machines;
pub mod map;
pub mod notes;
pub mod project;
pub mod read;
pub mod rename;
