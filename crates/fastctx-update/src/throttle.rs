//! A ceiling on how fast a hub takes packages in, shared by every byte it
//! takes (`world-network.md`, 中枢替网络存的东西): a cloud server's guard
//! against floods judges by how fast traffic comes in, and the hub knows
//! nothing of the server it runs on, so it takes a release in at a steady
//! pace any server can take, never in a rush.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// How far ahead of its pace the ceiling lets bytes come at once.
const BURST: Duration = Duration::from_millis(200);

pub struct Throttle {
    /// Bytes a second.
    rate: AtomicU64,
    /// When the bytes let through so far are paid for, at the rate.
    paid: Mutex<Instant>,
    /// Every byte let through, for how fast it goes now.
    taken: AtomicU64,
}

impl Throttle {
    pub fn new(rate: u64) -> Self {
        Self { rate: AtomicU64::new(rate.max(1)), paid: Mutex::new(Instant::now()), taken: AtomicU64::new(0) }
    }

    /// Bytes a second from now on.
    pub fn set(&self, rate: u64) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Every byte let through so far.
    pub fn taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    /// Waits until `bytes` more may come, in the order asked.
    pub async fn take(&self, bytes: u64) {
        let wake = {
            let mut paid = self.paid.lock().await;
            let now = Instant::now();
            // Time left unused is not saved up: only `BURST` runs ahead.
            *paid = (*paid).max(now);
            *paid += Duration::from_secs_f64(bytes as f64 / self.rate() as f64);
            paid.checked_sub(BURST).unwrap_or(*paid)
        };
        self.taken.fetch_add(bytes, Ordering::Relaxed);
        tokio::time::sleep_until(wake).await;
    }
}
