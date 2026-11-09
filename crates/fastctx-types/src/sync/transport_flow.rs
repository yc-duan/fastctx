//! Pure byte-window accounting and bounded metadata assembly.
//!
//! Storage checkpointing and durable cancellation belong to the receiver's
//! runtime. Neither consuming credit nor finishing assembly is a DurableAck.

use super::{
    cursor::ByteCount,
    session::*,
    transport_codec::{CodecError, validate_metadata},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

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

/// Share one reservation pool across all receiving sessions. Frame buffers and
/// downstream queues must reserve from the same process budget separately.
#[derive(Clone, Debug)]
pub struct MetadataBudget(Arc<Budget>);
#[derive(Debug)]
struct Budget {
    limit: usize,
    used: AtomicUsize,
}
impl MetadataBudget {
    pub fn new(bytes: usize) -> Self {
        Self(Arc::new(Budget { limit: bytes, used: AtomicUsize::new(0) }))
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }
    fn reserve(&self, bytes: usize) -> Result<Reservation, CodecError> {
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.0.limit)
            })
            .map_err(|_| CodecError::Busy)?;
        Ok(Reservation { budget: self.clone(), bytes })
    }
}
#[derive(Debug)]
struct Reservation {
    budget: MetadataBudget,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Owns its reservation until consumed/dropped, including after leaving the
/// assembler. Moving bytes to another queue does not silently free the budget.
#[derive(Debug)]
pub struct CompletedMetadata {
    pub message_id: MessageId,
    bytes: Vec<u8>,
    _reservation: AssemblyReservation,
}
impl CompletedMetadata {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
#[derive(Debug)]
struct AssemblyReservation {
    _global: Reservation,
    _session: Reservation,
}
struct Assembly {
    bytes: Vec<u8>,
    total: usize,
    reservation: AssemblyReservation,
}

/// Interleaved messages are bounded by count, the shared byte pool, and a session
/// share of at most one eighth of that pool. Completed queued messages retain
/// both reservations. A stalled session cannot reserve the entire process pool.
/// Offsets are contiguous; any gap/replay is rejected without changing state.
/// A caller may return frame credit after `push`, before the message is whole.
pub struct MetadataReassembly {
    session: SessionId,
    limits: TransportLimits,
    budget: MetadataBudget,
    session_budget: MetadataBudget,
    active: BTreeMap<MessageId, Assembly>,
}
impl MetadataReassembly {
    pub fn new(session: SessionId, limits: TransportLimits, budget: MetadataBudget) -> Result<Self, CodecError> {
        limits.validate().map_err(CodecError::InvalidFrame)?;
        let session_bytes = budget.0.limit / 8;
        if session_bytes < limits.max_metadata_bytes as usize {
            return Err(CodecError::InvalidFrame("metadata pool is too small for the negotiated page bound".into()));
        }
        Ok(Self {
            session,
            limits,
            budget,
            session_budget: MetadataBudget::new(session_bytes),
            active: BTreeMap::new(),
        })
    }
    pub fn push(&mut self, header: &MetadataPart, payload: &[u8]) -> Result<Option<CompletedMetadata>, CodecError> {
        if header.session != self.session {
            return Err(CodecError::InvalidFrame("metadata from another session".into()));
        }
        validate_metadata(header, payload.len(), &self.limits)?;
        if !self.active.contains_key(&header.message_id) {
            if header.offset.0 != 0 {
                return Err(CodecError::InvalidFrame("metadata offset gap".into()));
            }
            if self.active.len() >= self.limits.max_active_parts as usize {
                return Err(CodecError::Busy);
            }
            let total = header.total_bytes.0 as usize;
            let session = self.session_budget.reserve(total)?;
            let reservation = AssemblyReservation { _global: self.budget.reserve(total)?, _session: session };
            self.active
                .insert(header.message_id.clone(), Assembly { bytes: Vec::with_capacity(total), total, reservation });
        }
        let assembly = self.active.get_mut(&header.message_id).expect("inserted assembly");
        if header.total_bytes.0 != assembly.total as u64 || header.offset.0 != assembly.bytes.len() as u64 {
            return Err(CodecError::InvalidFrame("metadata offset or total changed".into()));
        }
        assembly.bytes.extend_from_slice(payload);
        if assembly.bytes.len() != assembly.total {
            return Ok(None);
        }
        let assembly = self.active.remove(&header.message_id).expect("completed assembly");
        Ok(Some(CompletedMetadata {
            message_id: header.message_id.clone(),
            bytes: assembly.bytes,
            _reservation: assembly.reservation,
        }))
    }
    /// Abort frees only this message's temporary memory, never an asset.
    pub fn cancel(&mut self, message: &MessageId) {
        self.active.remove(message);
    }
}
