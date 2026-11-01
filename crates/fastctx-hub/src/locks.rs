//! The hub's shared in-memory locks, registered with the process's order of
//! locks and wait graph (`fastctx_platform::doing`), each timed as it is
//! waited for and held.

use crate::measure::thread_cpu;
use fastctx_platform::doing::{MemoryMutex, MemoryMutexGuard, Rank};
use std::convert::Infallible;
use std::ops::{Deref, DerefMut};
use std::time::Instant;

pub(crate) struct Mutex<T> {
    name: &'static str,
    inner: MemoryMutex<T>,
    waiting: crate::measure::Histogram,
    holding: crate::measure::Histogram,
    holding_cpu: crate::measure::Histogram,
}

impl<T> Mutex<T> {
    pub(crate) fn new(name: &'static str, rank: Rank, value: T) -> Self {
        Self {
            name,
            inner: MemoryMutex::shared(rank, name, value),
            waiting: Default::default(),
            holding: Default::default(),
            holding_cpu: Default::default(),
        }
    }
    #[track_caller]
    pub(crate) fn lock(&self) -> Result<Guard<'_, T>, Infallible> {
        let began = Instant::now();
        let inner = self.inner.lock();
        self.waiting.record(began.elapsed());
        Ok(Guard {
            inner,
            began: Instant::now(),
            cpu_began: thread_cpu(),
            holding: &self.holding,
            holding_cpu: &self.holding_cpu,
        })
    }
    pub(crate) fn measurements(&self) -> serde_json::Value {
        serde_json::json!({ "name": self.name, "waiting": self.waiting.snapshot(), "holding": self.holding.snapshot(), "holding_cpu": self.holding_cpu.snapshot() })
    }
}

pub(crate) struct Guard<'a, T> {
    inner: MemoryMutexGuard<'a, T>,
    began: Instant,
    holding: &'a crate::measure::Histogram,
    cpu_began: Option<std::time::Duration>,
    holding_cpu: &'a crate::measure::Histogram,
}
impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}
impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.holding.record(self.began.elapsed());
        if let Some(elapsed) = self.cpu_began.zip(thread_cpu()).and_then(|(before, after)| after.checked_sub(before)) {
            self.holding_cpu.record(elapsed);
        }
    }
}
