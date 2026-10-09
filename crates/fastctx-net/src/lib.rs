//! Machines and the hub (`world-network.md`).
//!
//! Every computer or phone logged in to a hub keeps one connection to it and
//! says everything through it; the hub knows who is who and what each may
//! do (`directory`), and passes on what members say to each other only when
//! the directory allows it. This crate holds what both sides share: keys
//! and signatures, logins and enrollment tokens, the directory and its
//! judgments, the frames on the wire, and each machine's side of the
//! connection.

#[cfg(target_os = "android")]
pub mod android;
pub mod arrivals;
pub mod data;
pub mod dial;
pub mod directory;
pub mod envelope;
pub mod file_read;
pub mod file_search;
pub mod ids;
pub mod ledger;
pub mod link;
pub mod login;
pub mod message;
pub mod proxy;
pub mod route;
pub mod sign;
pub mod tls;
pub mod token;
pub mod wire;
