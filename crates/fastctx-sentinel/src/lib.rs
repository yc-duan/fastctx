//! Sentinels use the internal agent runtime without depending on a node.
pub mod ports;
pub mod reflex;
pub mod records;
pub mod settings;
pub mod state;
pub mod wake;
pub mod web;
pub mod words;
pub use fastctx_types::sentinel::*;
pub use wake::wake;
