//! Signed watch packets share a connection at bounded byte quanta. Large history
//! packets can interleave with other subscriptions without weakening verification.

pub mod commands;
mod reader;
mod writer;
pub use reader::{Reader, Received};
pub use writer::{Encoded, Output, encode, write};

use crate::sign::Signature;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const SUBSCRIPTIONS: usize = 256;
// Retiring packets coexist briefly with their replacement subscriptions.
const INFLIGHT_PACKETS: usize = 2 * SUBSCRIPTIONS + 1;
pub const QUANTUM: usize = 16 * 1024;
pub const MAX_PACKET: usize = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Part {
    watch_mux: u32,
    subscription: u64,
    number: u64,
    offset: usize,
    total: usize,
    bytes: usize,
    signature: Signature,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    discard: bool,
}
