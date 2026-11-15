//! What the node's timeline is made with that needs nothing of the node:
//! the work store a whole reading is placed through (`timeline::spill`), the
//! names, identities and order of its entries, and the merging of a host's
//! record with the World's (`talk::merging`). Cut from `fastctx-node`, which
//! uses it by the same paths; it compiles beside the node's other crates.

pub mod talk;
pub mod timeline;
pub mod user_images;
