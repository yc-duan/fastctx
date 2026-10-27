//! Bounded observation queues. Replaceable states coalesce at their new position;
//! ordered events never coalesce. Overflow discards the queue and reports a gap,
//! so a consumer resumes from its durable cursor instead of accepting a false end.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone)]
pub struct Budget(Arc<Memory>);
struct Memory {
    used: AtomicUsize,
    limit: usize,
}
struct Reservation {
    budget: Budget,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
impl Budget {
    pub fn new(bytes: usize) -> Self {
        Self(Arc::new(Memory { used: AtomicUsize::new(0), limit: bytes }))
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }
    fn reserve(&self, bytes: usize) -> Option<Reservation> {
        self.0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|total| *total <= self.0.limit)
            })
            .ok()
            .map(|_| Reservation { budget: self.clone(), bytes })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lagged;
impl std::fmt::Display for Lagged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the watch fell behind; resume it from the last received version")
    }
}
impl std::error::Error for Lagged {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryRecvError {
    Empty,
    Disconnected,
    Lagged,
}

pub struct Sender<T>(Arc<Shared<T>>);
pub struct Receiver<T>(Arc<Shared<T>>);
struct Shared<T> {
    state: Mutex<State<T>>,
    next: Notify,
    space: Notify,
    closed: Notify,
    senders: AtomicUsize,
    frames: usize,
    bytes: usize,
    single: usize,
    budget: Budget,
    measure: fn(&T) -> (usize, Option<String>),
}
struct State<T> {
    entries: BTreeMap<u64, Entry<T>>,
    keys: HashMap<String, u64>,
    sequence: u64,
    bytes: usize,
    receiver: bool,
    lagged: bool,
}
struct Entry<T> {
    value: T,
    key: Option<String>,
    memory: Reservation,
}

/// `single` permits one large initial snapshot in an otherwise empty queue.
/// It is still charged to the shared budget; it never exempts later events.
/// `measure` counts retained bytes and optionally identifies a complete state
/// that a newer state may replace. It must never classify a delta as a state.
pub fn channel<T>(
    frames: usize,
    bytes: usize,
    single: usize,
    budget: Budget,
    measure: fn(&T) -> (usize, Option<String>),
) -> (Sender<T>, Receiver<T>) {
    assert!(frames > 0 && bytes > 0 && single >= bytes);
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            entries: BTreeMap::new(),
            keys: HashMap::new(),
            sequence: 0,
            bytes: 0,
            receiver: true,
            lagged: false,
        }),
        next: Notify::new(),
        space: Notify::new(),
        closed: Notify::new(),
        senders: AtomicUsize::new(1),
        frames,
        bytes,
        single,
        budget,
        measure,
    });
    (Sender(shared.clone()), Receiver(shared))
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.0.senders.fetch_add(1, Ordering::Relaxed);
        Self(self.0.clone())
    }
}
impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.next.notify_one();
        }
    }
}
impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), Lagged> {
        let (size, key) = (self.0.measure)(&value);
        self.send_measured(value, size, key)
    }

    /// Reuses a measurement when publishing the same immutable value to many
    /// subscribers. Its size and replacement key must match the channel policy.
    pub fn send_measured(&self, value: T, size: usize, key: Option<String>) -> Result<(), Lagged> {
        let size = size.saturating_add(256).saturating_add(key.as_ref().map_or(0, String::len));
        let mut state = self.0.state.lock().expect("never poisoned");
        if !state.receiver {
            return Err(Lagged);
        }
        if let Some(previous) = key.as_ref().and_then(|key| state.keys.remove(key))
            && let Some(entry) = state.entries.remove(&previous)
        {
            state.bytes -= entry.memory.bytes;
        }
        let allowed = state.entries.len() < self.0.frames
            && size <= self.0.single
            && (state.entries.is_empty() || state.bytes.saturating_add(size) <= self.0.bytes);
        let reserved = allowed.then(|| self.0.budget.reserve(size)).flatten();
        let Some(memory) = reserved else {
            drop(state);
            self.lag();
            return Err(Lagged);
        };
        let Some(sequence) = state.sequence.checked_add(1) else {
            drop(state);
            self.lag();
            return Err(Lagged);
        };
        state.sequence = sequence;
        state.bytes += size;
        if let Some(key) = &key {
            state.keys.insert(key.clone(), sequence);
        }
        state.entries.insert(sequence, Entry { value, key, memory });
        drop(state);
        self.0.next.notify_one();
        Ok(())
    }

    /// Backpressure for initial history, whose delivery need not keep pace with
    /// live publication. The waiting value is charged to the shared byte budget.
    /// Live producers use `send` so a slow observer cannot stall publication.
    pub async fn send_wait(&self, value: T, within: std::time::Duration) -> Result<(), Lagged> {
        let (size, key) = (self.0.measure)(&value);
        let size = size.saturating_add(256).saturating_add(key.as_ref().map_or(0, String::len));
        let memory = (size <= self.0.single).then(|| self.0.budget.reserve(size)).flatten();
        let Some(memory) = memory else {
            self.lag();
            return Err(Lagged);
        };
        let sending = async {
            loop {
                let space = self.0.space.notified();
                tokio::pin!(space);
                space.as_mut().enable();
                {
                    let mut state = self.0.state.lock().expect("never poisoned");
                    if !state.receiver {
                        return Err(Lagged);
                    }
                    let prior = key.as_ref().and_then(|key| state.keys.get(key)).copied();
                    let old_bytes = prior.and_then(|n| state.entries.get(&n)).map_or(0, |entry| entry.memory.bytes);
                    let frames = state.entries.len() - usize::from(prior.is_some());
                    if frames < self.0.frames
                        && (frames == 0 || (state.bytes - old_bytes).saturating_add(size) <= self.0.bytes)
                    {
                        if let Some(n) = prior {
                            state.entries.remove(&n);
                        }
                        let Some(sequence) = state.sequence.checked_add(1) else {
                            return Err(Lagged);
                        };
                        state.sequence = sequence;
                        state.bytes = state.bytes - old_bytes + size;
                        if let Some(key) = &key {
                            state.keys.insert(key.clone(), sequence);
                        }
                        state.entries.insert(sequence, Entry { value, key, memory });
                        drop(state);
                        self.0.next.notify_one();
                        return Ok(());
                    }
                }
                space.await;
            }
        };
        match tokio::time::timeout(within, sending).await {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.lag();
                Err(Lagged)
            }
        }
    }

    /// Propagates an upstream gap without disguising it as a normal end.
    pub fn lag(&self) {
        let mut state = self.0.state.lock().expect("never poisoned");
        if state.receiver {
            state.receiver = false;
            state.lagged = true;
            state.entries.clear();
            state.keys.clear();
            state.bytes = 0;
        }
        drop(state);
        self.0.next.notify_one();
        self.0.space.notify_waiters();
        self.0.closed.notify_waiters();
    }
    pub fn is_closed(&self) -> bool {
        !self.0.state.lock().expect("never poisoned").receiver
    }
    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub async fn closed(&self) {
        loop {
            let changed = self.0.closed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_closed() {
                return;
            }
            changed.await;
        }
    }
}

impl<T> Receiver<T> {
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut state = self.0.state.lock().expect("never poisoned");
        if std::mem::take(&mut state.lagged) {
            return Err(TryRecvError::Lagged);
        }
        if let Some((_, entry)) = state.entries.pop_first() {
            state.bytes -= entry.memory.bytes;
            if let Some(key) = entry.key {
                state.keys.remove(&key);
            }
            self.0.space.notify_waiters();
            return Ok(entry.value);
        }
        if !state.receiver || self.0.senders.load(Ordering::Acquire) == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
    /// Cancellation-safe: only a returned event has been removed from the queue.
    pub async fn recv(&mut self) -> Result<Option<T>, Lagged> {
        loop {
            let shared = self.0.clone();
            let changed = shared.next.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_recv() {
                Ok(value) => return Ok(Some(value)),
                Err(TryRecvError::Disconnected) => return Ok(None),
                Err(TryRecvError::Lagged) => return Err(Lagged),
                Err(TryRecvError::Empty) => changed.await,
            }
        }
    }
}
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().expect("never poisoned");
        state.receiver = false;
        state.entries.clear();
        state.keys.clear();
        state.bytes = 0;
        drop(state);
        self.0.space.notify_waiters();
        self.0.closed.notify_waiters();
    }
}
