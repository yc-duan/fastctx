//! Node-wide monotonic online time, independent of any conversation's RPC latency.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::policy::ONLINE_GAP_MS;

pub(super) struct Uptime(Mutex<Sample>);

impl Default for Uptime {
    fn default() -> Self {
        Self(Mutex::new(Sample { at: Instant::now(), online: Duration::ZERO, generation: 0 }))
    }
}

pub(super) struct Reading {
    pub online_ms: u64,
    pub generation: u64,
}

impl Uptime {
    pub(super) fn read(&self) -> Reading {
        self.0.lock().expect("goal uptime").sample(Instant::now())
    }
}

struct Sample {
    at: Instant,
    online: Duration,
    generation: u64,
}

impl Sample {
    fn sample(&mut self, now: Instant) -> Reading {
        let elapsed = now.duration_since(self.at);
        if elapsed > Duration::from_millis(ONLINE_GAP_MS) {
            self.generation = self.generation.wrapping_add(1);
        } else {
            self.online = self.online.saturating_add(elapsed);
        }
        self.at = now;
        Reading { online_ms: self.online.as_millis().min(u128::from(u64::MAX)) as u64, generation: self.generation }
    }
}
