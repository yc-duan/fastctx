//! Sentinels use the internal agent runtime without depending on a node.
pub mod goal;
pub mod ports;
pub mod lifecycle;
pub mod record;
pub mod reflex;
pub mod records;
pub mod settings;
pub mod state;
pub mod wake;
pub mod web;
pub mod words;
pub use fastctx_types::sentinel::*;
pub use wake::wake;
pub mod resident;
