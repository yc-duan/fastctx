//! Loans split one admitted reservation; returning a loan never joins a queue.
use super::BytePermit;
use crate::metrics::{Cause, Counter, Metrics, PlatformMetrics};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// An operation's complete working set, admitted once before allocating buffers.
///
/// `take` only splits this reservation. It never acquires global permits. Keep
/// this owner alive while workspace will be reused. Dropping it releases idle
/// bytes immediately; outstanding loans remain charged until their real owners
/// (including cancelled callers' continuing IO) drop them.
#[derive(Debug)]
pub struct BudgetWindow {
    capacity: usize,
    reserve: Arc<Mutex<OwnedSemaphorePermit>>,
}

/// An internal cost-model error, not transient pool contention. Debug builds
/// also assert at the failing `take`, after recording the fixed metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowExhausted {
    pub requested: usize,
    pub available: usize,
}
impl std::fmt::Display for WindowExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "working-set cost exceeded its window: requested {}, available {}", self.requested, self.available)
    }
}
impl std::error::Error for WindowExhausted {}

pub(super) type ReturnTo = Weak<Mutex<OwnedSemaphorePermit>>;

impl BudgetWindow {
    pub(super) fn new(permit: OwnedSemaphorePermit) -> Self {
        Self { capacity: permit.num_permits(), reserve: Arc::new(Mutex::new(permit)) }
    }
    pub(super) fn belongs_to(&self, semaphore: &Arc<Semaphore>) -> bool {
        let reserve = self.reserve.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::ptr_eq(reserve.semaphore(), semaphore)
    }
    /// The total admitted bytes, including outstanding loans.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    /// Idle bytes that `take` can borrow now. This is not global free capacity.
    pub fn available(&self) -> usize {
        self.reserve.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).num_permits()
    }
    /// Borrows bytes without waiting or changing the global reservation.
    ///
    /// Release earlier loans before reusing their workspace. An insufficient
    /// window is an implementation error; retrying it as Busy is incorrect.
    pub fn take(&self, bytes: usize) -> Result<BytePermit, WindowExhausted> {
        let (permit, available) = {
            let mut reserve = self.reserve.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let available = reserve.num_permits();
            (reserve.split(bytes), available)
        };
        if let Some(permit) = permit {
            return Ok(BytePermit { permit: Some(permit), return_to: Some(Arc::downgrade(&self.reserve)) });
        }
        PlatformMetrics.add(Counter::BudgetWindowExhausted, Cause::Integrity, 1);
        debug_assert!(
            bytes <= available,
            "working-set cost exceeded its window: requested {bytes}, available {available}"
        );
        Err(WindowExhausted { requested: bytes, available })
    }
}

pub(super) fn return_loan(permit: OwnedSemaphorePermit, target: Option<&ReturnTo>) {
    if let Some(reserve) = target.and_then(Weak::upgrade) {
        reserve.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).merge(permit);
    }
    // Without a live window, the owned semaphore permit returns to the pool.
}
