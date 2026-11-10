//! Shared allocations in the canonical source's HostRows family. Host and World
//! adapters load and replace this row under the same issued destination step.
use crate::{cursor::Sequence, store::Error};
use serde::{Deserialize, Serialize};

pub const CLOCK: &str = "conversation-order-v1";

/// Persistent counters retain their original JSON field names and numeric shape.
/// Display order is independent of these creation and evidence identities.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Clock {
    pub accepted: u64,
    pub created: u64,
    pub turns: u64,
}
impl Clock {
    pub fn accept(&mut self) -> Result<Sequence, Error> {
        increment(&mut self.accepted)
    }
    pub fn create(&mut self) -> Result<Sequence, Error> {
        increment(&mut self.created)
    }
    pub fn turn(&mut self) -> Result<Sequence, Error> {
        increment(&mut self.turns)
    }
}
fn increment(n: &mut u64) -> Result<Sequence, Error> {
    *n = n.checked_add(1).ok_or_else(|| Error::Invalid("canonical allocation exhausted".into()))?;
    Ok(Sequence(*n))
}
