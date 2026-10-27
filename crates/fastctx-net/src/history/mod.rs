//! A session's history as it goes to a viewer that keeps it
//! (`fastctx_types::session::held`): slimmed to what an interface draws,
//! cut into segments named by their content, and packed.

mod pack;
mod segments;
mod slim;

pub use pack::{pack, unpack};
pub use segments::{Cut, cut, pieces, split, turn_boundaries};
pub use slim::{Pictures, fingerprint, slim, transient};
