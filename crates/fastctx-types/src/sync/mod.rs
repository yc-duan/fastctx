//! Account asset identities and replication contracts.
//!
//! These types do not advertise a capability or enable a transport. A service
//! may advertise them only after installing its durable storage and handlers.

pub mod catalog;
pub mod cursor;
pub mod exposure;
pub mod identity;
pub mod manifest;
pub mod mutation;
pub mod proof;
pub mod timeline;
pub mod transfer;

/// The proposed first generation, advertised only by a complete implementation.
pub const PROTOCOL: u16 = 1;
/// A storage block's maximum decoded length; transport frames may be smaller.
pub const BLOCK_BYTES: u32 = 1024 * 1024;
/// Metadata is paged independently of the size of the referenced assets.
pub const PAGE_BYTES: u32 = 256 * 1024;
