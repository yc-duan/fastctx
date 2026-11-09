//! Adapters from host decoders to registered source writer plans. Storage and
//! authorization stay on the node side of the host-format boundary.

pub mod bodies;
pub mod checkpoint_bundle;
pub mod checkpoints;
pub mod display;
pub mod display_pages;
pub mod facts;
pub mod parts;
pub mod plan;
pub mod resupply;
pub mod state_read;
pub mod step_content;
