//! Reserved admission per priority, with weighted service to prevent starvation.

use crate::writer::Job;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Transport receipts precede interactive writes, observations and maintenance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    Control = 0,
    Interactive = 1,
    #[default]
    Normal = 2,
    Background = 3,
}

// Admission is reserved, so background payloads cannot occupy interactive slots.
const SLOTS: usize = 256;
const BYTES: [usize; 4] = [16 << 20, 16 << 20, 64 << 20, 16 << 20];
const SCHEDULE: [usize; 15] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3];

#[derive(Default)]
struct State {
    rows: [VecDeque<Job>; 4],
    next: usize,
    closed: bool,
}

pub(super) struct Queue {
    state: Mutex<State>,
    changed: Condvar,
    slots: [Arc<Semaphore>; 4],
    bytes: [Arc<Semaphore>; 4],
}

pub(super) struct Admission {
    _slot: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            slots: std::array::from_fn(|_| Arc::new(Semaphore::new(SLOTS))),
            bytes: std::array::from_fn(|i| Arc::new(Semaphore::new(BYTES[i]))),
        }
    }

    pub async fn admit(&self, priority: Priority, bytes: usize) -> crate::Result<Admission> {
        let amount = u32::try_from(bytes.saturating_add(512))
            .ok()
            .filter(|n| *n <= BYTES[priority as usize] as u32)
            .ok_or_else(|| {
                crate::Error::with_kind(crate::ErrorKind::Invalid, "the database operation exceeds its input budget")
            })?;
        let at = priority as usize;
        let slot = self.slots[at].clone().acquire_owned().await.map_err(|_| stopped())?;
        let bytes = self.bytes[at].clone().acquire_many_owned(amount).await.map_err(|_| stopped())?;
        Ok(Admission { _slot: slot, _bytes: bytes })
    }

    pub fn push(&self, priority: Priority, job: Job) -> crate::Result<()> {
        let mut state = self.state.lock().expect("writer queue");
        if state.closed {
            return Err(stopped());
        }
        state.rows[priority as usize].push_back(job);
        self.changed.notify_one();
        Ok(())
    }

    pub fn pop(&self, idle: Duration) -> Option<Job> {
        let mut state = self.state.lock().expect("writer queue");
        if state.rows.iter().all(VecDeque::is_empty) && !state.closed {
            state = self
                .changed
                .wait_timeout_while(state, idle, |state| state.rows.iter().all(VecDeque::is_empty) && !state.closed)
                .expect("writer queue")
                .0;
        }
        for _ in 0..SCHEDULE.len() {
            let priority = SCHEDULE[state.next];
            state.next = (state.next + 1) % SCHEDULE.len();
            if let Some(job) = state.rows[priority].pop_front() {
                return Some(job);
            }
        }
        None
    }

    pub fn finished(&self) -> bool {
        let state = self.state.lock().expect("writer queue");
        state.closed && state.rows.iter().all(VecDeque::is_empty)
    }

    pub fn close(&self) {
        self.state.lock().expect("writer queue").closed = true;
        for semaphore in self.slots.iter().chain(&self.bytes) {
            semaphore.close();
        }
        self.changed.notify_one();
    }
}

fn stopped() -> crate::Error {
    crate::Error::with_kind(crate::ErrorKind::Unavailable, "the database writer stopped")
}
