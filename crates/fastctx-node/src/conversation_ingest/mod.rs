//! Adapters from host decoders to registered source writer plans. Storage and
//! authorization stay on the node side of the host-format boundary.

pub mod checkpoint_bundle;
pub mod plan;
pub mod state_read;
