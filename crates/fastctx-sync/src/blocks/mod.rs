//! Bounded content validation precedes publication and durable reference changes.

mod access;
mod adopted;
pub(crate) mod authority_supply;
mod budget;
pub(crate) use budget::Permit;

pub mod codec;
mod files;
mod inline;
pub use inline::INLINE_BYTES;
mod leases;
mod location;
pub mod manifest;
pub(crate) mod metadata;
mod pages;
mod pins;

pub(crate) mod projection_commit;
pub(crate) mod reconcile;
pub(crate) mod replica_commit;
mod root_holds;
mod rows;
pub(crate) mod source_commit;
pub(crate) mod source_supply;
mod source_transfer;
mod sqlite;
mod steps;
pub(crate) mod supply;
mod tasks;
mod uploads;
pub use sqlite::SqliteBlocks;
pub use steps::ReconcileCursor;
pub(crate) use steps::STEP_TARGETS;
