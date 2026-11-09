//! Account asset identities and replication contracts.
//!
//! These types do not advertise a capability or enable a transport. A service
//! may advertise them only after installing its durable storage and handlers.

pub mod acceptance;
mod canonical;
pub mod catalog;
pub mod conversation_identity;
pub mod conversation_timeline;
pub mod cursor;
pub mod exposure;
pub mod feed;
pub mod identity;
pub mod manifest;
pub mod metadata;
pub mod mutation;
pub mod progress;
pub mod proof;
pub mod scope;
pub mod session;
pub mod timeline;
pub mod transfer;
pub mod transport_codec;
pub mod transport_flow;

/// The proposed first generation, advertised only by a complete implementation.
pub const PROTOCOL: u16 = 1;
/// A storage block's maximum decoded length; transport frames may be smaller.
pub const BLOCK_BYTES: u32 = 1024 * 1024;
/// Metadata is paged independently of the size of the referenced assets.
pub const PAGE_BYTES: u32 = 256 * 1024;
/// Content roots one source record may own beyond its payload block.
pub const RECORD_ROOTS: usize = 64;
