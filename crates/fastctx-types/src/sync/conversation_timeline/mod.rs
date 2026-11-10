//! Schema 1 of `conversation.timeline`, shared by every host and legacy import.
//!
//! Source records retain observations and their original content before display
//! projection. The canonical producer emits item revisions; replicas install
//! those revisions instead of independently merging observations. `accepted`
//! and creation coordinates are allocated by that producer and survive replay.
//! They are never timestamps or counters assigned by a receiving replica.
//! Creation keys stay unique within the canonical conversation across producer
//! handovers; a new ProducerEpoch must not restart display-key allocation.
//!
//! Content references here own only this conversation's permitted bytes. Jobs,
//! independent pictures and child conversations are logical ObjectKey links,
//! not permission to install another object's root. Source signature, authority,
//! scope and alias-receipt verification precede the pure reducer.

mod body;
mod display;
mod display_validation;
mod facts;
mod facts_validation;
pub use display::*;
pub use facts::*;
mod fact;
mod order;
pub use body::*;
pub use fact::*;
pub use order::*;

pub const KIND: &str = "conversation.timeline";
pub const SCHEMA: u16 = 1;
