//! Opening again what broke (`world-network.md`, 断开不是结束). A stream an
//! interface still holds is never over because its connection broke: it is
//! opened again at once, then after waits that double up to a minute, each a
//! random part of that, as the link to the hub waits; once it stayed up for a
//! minute, the next break starts over from at once. A wait ends early when
//! this device is told to try now: its network changed, it woke, an
//! interface came to the front (`World::retry_now`).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use fastctx_types::MachineName;
use tokio::sync::Notify;

use crate::World;

/// A stream that stays up this long counts as healthy, as a link does.
const STABLE: Duration = Duration::from_secs(60);

/// A stream that broke and is being opened again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interruption {
    /// Why it broke, or why opening it again failed last.
    pub why: String,
    /// When it first broke; tries that fail since do not move it.
    pub since: SystemTime,
}

/// How one stream is paced while it is opened again and again; every
/// stream an interface holds open is paced by one of these.
#[derive(Default)]
pub struct Again {
    tries: u32,
    opened: Option<Instant>,
    since: Option<SystemTime>,
}

impl Again {
    /// It broke, or opening it again failed, for `why`.
    pub fn broke(&mut self, why: String) -> Interruption {
        if self.opened.take().is_some_and(|at| at.elapsed() >= STABLE) {
            self.tries = 0;
        }
        self.tries = self.tries.saturating_add(1);
        let since = *self.since.get_or_insert_with(SystemTime::now);
        Interruption { why, since }
    }

    /// Something came on it again; returns whether it had broken.
    pub fn came(&mut self) -> bool {
        if self.opened.is_none() {
            self.opened = Some(Instant::now());
        }
        self.since.take().is_some()
    }

    /// The wait before the next try, as a future of its own: it ends after
    /// the backoff, or as soon as this device is told to try now or, on a
    /// device that holds its own links, the link that reaches `machine`
    /// changes.
    pub fn pause(&self, world: &World, machine: Option<&MachineName>) -> impl Future<Output = ()> + Send + 'static {
        let wait = fastctx_net::dial::backoff(self.tries);
        let told: Arc<Notify> = world.retry.clone();
        let mut link = world.device().and_then(|device| device.machine_changes(machine).ok()).map(|(_, link)| link);
        async move {
            let now = told.notified();
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = now => {}
                () = async {
                    match link.as_mut() {
                        Some(link) => { let _ = link.changed().await; }
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
    }
}
