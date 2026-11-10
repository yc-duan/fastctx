//! Bounded metadata assembly uses the same runtime byte admission as frame IO.
pub use crate::admission::ByteBudget as MetadataBudget;
use crate::admission::BytePermit;
use fastctx_types::sync::{
    session::*,
    transport_codec::{CodecError, validate_metadata},
};
use std::collections::BTreeMap;

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
    _global: BytePermit,
    _session: BytePermit,
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
        let session_bytes = budget.capacity() / 8;
        if session_bytes < limits.max_metadata_bytes as usize {
            return Err(CodecError::InvalidFrame("metadata pool is too small for the negotiated page bound".into()));
        }
        Ok(Self {
            session,
            limits,
            budget,
            session_budget: MetadataBudget::new(session_bytes).map_err(|e| CodecError::InvalidFrame(e.to_string()))?,
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
            let session = self.session_budget.try_acquire(total).map_err(|_| CodecError::Busy)?;
            let reservation = AssemblyReservation {
                _global: self.budget.try_acquire(total).map_err(|_| CodecError::Busy)?,
                _session: session,
            };
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

pub use crate::replicate::{decode_json, decode_message};
