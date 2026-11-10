//! Adapters from host decoders to registered source writer plans. Storage and
//! authorization stay on the node side of the host-format boundary.

pub mod bodies;
pub mod checkpoint_bundle;
pub mod checkpoints;
mod claim;
pub mod display;
mod display_append;
pub mod display_pages;
mod engine;
pub mod facts;
mod indexes;
pub mod item_keys;
pub mod parts;
pub mod plan;
mod publisher;
pub mod reading;
mod render;
pub mod resupply;
mod row_content;
mod row_index;
mod row_state;
pub mod source_io;

pub mod state_read;
pub mod step_content;
pub(crate) mod watcher;
