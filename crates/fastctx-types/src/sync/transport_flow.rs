//! Pure cumulative byte-window accounting.
//!
//! Storage checkpointing and durable cancellation belong to the receiver's
//! runtime. Neither consuming credit nor finishing assembly is a DurableAck.

use super::{cursor::ByteCount, session::*};
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    #[error("credit belongs to another session or stream")]
    WrongStream,
    #[error("payload exceeds granted credit")]
    NoCredit,
    #[error("credit counter exhausted; open a new session")]
    CounterExhausted,
}

/// One direction of one session/stream. Never shared between sessions or reused
/// after reconnect. Equal/older cumulative credits cannot create extra credit.
#[derive(Debug)]
pub struct CreditWindow {
    session: SessionId,
    stream: CreditStream,
    allowed: u64,
    consumed: u64,
}
impl CreditWindow {
    pub fn new(session: SessionId, stream: CreditStream) -> Self {
        Self { session, stream, allowed: 0, consumed: 0 }
    }
    pub fn apply(&mut self, credit: &Credit) -> Result<(), FlowError> {
        if credit.session != self.session || credit.stream != self.stream {
            return Err(FlowError::WrongStream);
        }
        self.allowed = self.allowed.max(credit.allowed_payload_total.0);
        Ok(())
    }
    /// Receiver-side increase, after acquiring or releasing the buffer budget.
    pub fn grant(&mut self, bytes: u32) -> Result<Credit, FlowError> {
        self.allowed = self.allowed.checked_add(u64::from(bytes)).ok_or(FlowError::CounterExhausted)?;
        Ok(Credit {
            session: self.session.clone(),
            stream: self.stream.clone(),
            allowed_payload_total: ByteCount(self.allowed),
        })
    }
    pub fn available(&self) -> ByteCount {
        ByteCount(self.allowed - self.consumed)
    }
    /// A partial frame is useful; do not wait for a whole storage block/window.
    pub fn next_payload(&self, frame_capacity: u32) -> u32 {
        self.available().0.min(u64::from(frame_capacity)) as u32
    }
    pub fn consume(&mut self, bytes: u32) -> Result<(), FlowError> {
        let next = self.consumed.checked_add(u64::from(bytes)).ok_or(FlowError::CounterExhausted)?;
        if next > self.allowed {
            return Err(FlowError::NoCredit);
        }
        self.consumed = next;
        Ok(())
    }
}
