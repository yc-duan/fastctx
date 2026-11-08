//! Sentinels use the internal agent runtime without depending on a node.
pub mod goal;
pub mod lifecycle;
mod page_text;
pub mod ports;
pub mod record;
pub mod records;
pub mod reflex;
pub mod settings;
pub mod state;
mod tools;
pub mod wake;
pub mod web;
pub mod words;
pub use fastctx_types::sentinel::*;
pub use wake::wake;
pub mod resident;
