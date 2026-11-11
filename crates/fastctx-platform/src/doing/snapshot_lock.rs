//! Diagnostic reads share one short deadline across independent registries.

use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

/// Waits on a blocking worker until the common deadline. Poisoning does not
/// discard the diagnostic state that can explain the failed owner.
pub fn snapshot_lock<T>(mutex: &Mutex<T>, deadline: Instant) -> Option<MutexGuard<'_, T>> {
    loop {
        match mutex.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(TryLockError::WouldBlock) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                std::thread::sleep(left.min(Duration::from_millis(2)));
            }
        }
    }
}
