//! Reclaims unpaired registrations without relying on another connection
//! arriving, and ties a waiting end or relay to the task that owns it.

use std::sync::Weak;
use std::time::Duration;

use tokio::time::Instant;

use crate::Hub;

/// How often expired registrations are removed, independently of the
/// interval at which health is reported.
const EVERY: Duration = Duration::from_secs(1);

/// Reclaims registrations whose ends did not pair in time. A relay owns
/// its registration until its ends close, however long that takes.
pub(crate) async fn expire(hub: Weak<Hub>) {
    let reason = format!("the data connection was not paired within {} s of registration", super::PAIRING.as_secs());
    loop {
        tokio::time::sleep(EVERY).await;
        let Some(hub) = hub.upgrade() else { return };
        let expired = {
            let mut pairing = hub.data.lock().expect("never poisoned");
            let now = Instant::now();
            let streams: Vec<u64> = pairing
                .registered
                .iter()
                .filter(|(stream, registration)| registration.deadline <= now && !pairing.live.contains_key(stream))
                .map(|(stream, _)| *stream)
                .collect();
            for stream in &streams {
                pairing.forget(*stream, &reason);
            }
            streams.len()
        };
        if expired > 0 {
            hub.log.line(format_args!("{expired} data connection registrations expired before pairing"));
        }
    }
}

/// The first end owns the wait and, after handover, the relay. Dropping its
/// future, including when the hub moves away, removes only this attempt.
pub(super) struct Active<'a> {
    hub: &'a Hub,
    stream: u64,
    serial: u64,
}

impl<'a> Active<'a> {
    pub(super) fn new(hub: &'a Hub, stream: u64, serial: u64) -> Self {
        Self { hub, stream, serial }
    }
}

impl Drop for Active<'_> {
    fn drop(&mut self) {
        let mut pairing = self.hub.data.lock().expect("never poisoned");
        if pairing.live.get(&self.stream).is_some_and(|live| live.serial == self.serial) {
            pairing.live.remove(&self.stream);
            pairing.registered.remove(&self.stream);
        } else {
            pairing.forget_waiting(self.stream, self.serial, "the waiting data connection ended");
        }
    }
}
