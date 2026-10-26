//! Machines and the hub (`world-network.md`).
//!
//! Every computer or phone logged in to a hub keeps one connection to it;
//! the hub knows who is who and what each may do (`directory`), and passes
//! on what members say to each other only when the directory allows it.
//! Machines the network's administrator allowed direct connections to are
//! also linked with directly, and what passes between two linked machines
//! does not go through the hub (`peer`). This crate holds what all sides
//! share: keys and signatures, logins and enrollment tokens, the directory
//! and its judgments, the frames on the wire, and each machine's side of
//! its connections.

pub mod afoot;
pub mod again;
#[cfg(target_os = "android")]
pub mod android;
pub mod around;
pub mod arrivals;
pub mod authorization;
pub mod call_details;
pub mod cancellable;
pub mod data;
pub mod dial;
pub mod directory;
pub mod envelope;
pub mod file_read;
pub mod file_search;
pub mod history;
pub mod ids;
pub mod ledger;
pub mod lines;
pub mod lines_read;
pub mod link;
pub mod listen;
pub mod login;
pub mod mcp_test;
pub mod memory;
pub mod message;
pub mod moving;
pub mod peer;
pub mod proxy;
pub mod pulse;
pub mod question_parts;
mod queue;
pub mod reassembly;
pub mod route;
pub mod sign;
pub mod tls;
pub mod token;
pub mod transport;
pub mod watch_stream;
pub mod web;
pub mod wire;
