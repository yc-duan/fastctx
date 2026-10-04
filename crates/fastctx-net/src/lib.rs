//! Machines and the hub (`world-network.md`).
//!
//! Every machine of a World keeps one connection to the hub and says
//! everything through it, each statement signed with the machine's own key.
//! This crate holds what both sides share: keys and signatures, the list of
//! members and how each machine checks it, the codes machines join with,
//! the frames on the wire, and each machine's side of the connection.

pub mod code;
pub mod data;
pub mod dial;
pub mod join;
pub mod ledger;
pub mod link;
pub mod message;
pub mod proxy;
pub mod route;
pub mod sign;
pub mod tls;
pub mod trust;
pub mod wire;
