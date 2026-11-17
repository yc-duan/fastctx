//! The registry of persistent data and the transitional preservation carrier.
//!
//! Every persistent table a node or hub keeps is named here by kind, so that
//! architecture checks and storage guards can refuse unregistered state.

pub mod format_upgrade;
pub mod migration;
pub mod registry;
