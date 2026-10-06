//! Machines and the hub (`world-network.md`).
//!
//! Every machine of a World keeps one connection to the hub and says
//! everything through it, each statement signed with the machine's own key
//! and everything it says to another member sealed for that member. This
//! crate holds what both sides share: keys, signatures and sealing, the
//! list of members, what each may do and how each machine checks it, the
//! codes machines join with, the frames on the wire, and each machine's
//! side of the connection.

#[cfg(target_os = "android")]
pub mod android;
pub mod code;
pub mod data;
pub mod dial;
pub mod envelope;
pub mod file_read;
pub mod join;
pub mod ledger;
pub mod link;
pub mod message;
pub mod noise;
pub mod proxy;
pub mod route;
pub mod scope;
pub mod seal;
pub mod sign;
pub mod stream_crypto;
pub mod tls;
pub mod tree;
pub mod trust;
pub mod wire;
