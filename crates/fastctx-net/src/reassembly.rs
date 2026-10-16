//! Process-wide admission for bodies assembled from remote pieces. A buffer
//! reserves its maximum before receiving anything and returns that reservation
//! when its owner finishes, fails or is cancelled.

use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The encoded body capacities and scratch allowances reserved by all
/// reassemblies together. This is not an allocator or whole-process RSS limit;
/// decoding and buffer reallocation can temporarily hold additional storage.
pub const BUDGET: usize = 128 * 1024 * 1024;

static RESERVED: AtomicUsize = AtomicUsize::new(0);

/// A reservation shared by hub answers, large calls and answers, and letters.
/// Keep it until all assembly storage covered by it has been dropped.
pub struct Reservation {
    bytes: usize,
}

impl Reservation {
    /// Reserves `bytes` immediately, or refuses without waiting for other work.
    pub fn new(bytes: usize) -> Result<Self, String> {
        RESERVED
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(bytes).filter(|next| *next <= BUDGET)
            })
            .map_err(|_| format!("reassembling remote bodies would exceed the {BUDGET}-byte process budget"))?;
        Ok(Self { bytes })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        RESERVED.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// A body whose capacity cannot outgrow its reservation. The reservation stays
/// with the returned bytes through decoding; there is no unaccounted Vec exit.
pub struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
    _reservation: Reservation,
}

impl Buffer {
    /// Reserves up to `limit` body bytes and `overhead` bytes of assembly scratch
    /// space. Storage is allocated only as bytes arrive.
    pub fn new(limit: usize, overhead: usize) -> Result<Self, String> {
        let reserved = limit.checked_add(overhead).ok_or("the reassembly size overflows")?;
        Ok(Self { bytes: Vec::new(), limit, _reservation: Reservation::new(reserved)? })
    }

    /// Drops the encoded bytes while retaining their reservation for the
    /// decoded body until its business handoff finishes.
    pub fn into_reservation(self) -> Reservation {
        self._reservation
    }

    /// Appends a piece, refusing before allocating if the body would be too big.
    pub fn extend(&mut self, piece: &[u8]) -> Result<(), String> {
        let length = self
            .bytes
            .len()
            .checked_add(piece.len())
            .filter(|length| *length <= self.limit)
            .ok_or_else(|| format!("the remote body exceeds its {}-byte limit", self.limit))?;
        if length > self.bytes.capacity() {
            let capacity = length.max(self.bytes.capacity().saturating_mul(2)).min(self.limit);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|error| format!("allocating a remote body: {error}"))?;
        }
        self.bytes.extend_from_slice(piece);
        Ok(())
    }
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}
