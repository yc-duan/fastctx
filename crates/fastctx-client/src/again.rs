//! Opening again what broke (`world-network.md`, 断开不是结束). A stream an
//! interface still holds is never over because its connection broke: it is
//! opened again at once, then after the waits everything that reconnects
//! keeps (`fastctx_net::again`); once it stayed up a while, the next break
//! starts over from at once. A wait ends early when this device is told to
//! try now, as when its network changed, it woke or an interface came to
//! the front (`World::retry_now`), and when a way to the machine the stream
//! comes from opens: through this computer's node, or on a device that
//! holds its own links, through them.

use std::sync::Arc;
use std::time::{Instant, SystemTime};

use fastctx_net::again::{STABLE, backoff};
use fastctx_types::access::Written;

use crate::World;

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

    /// The wait before the next try, as a future of its own (`pause`).
    pub fn pause(&self, world: &World, machine: Option<&Written>) -> impl Future<Output = ()> + Send + 'static {
        pause(world, self.tries, machine)
    }
}

/// The wait before try number `tries` (the first is 1) of what comes from
/// `machine`, as a future of its own: it ends after the wait, or as soon as
/// this device is told to try now or a way to `machine` opens.
pub fn pause(world: &World, tries: u32, machine: Option<&Written>) -> impl Future<Output = ()> + Send + 'static {
    let wait = backoff(tries);
    // Listening from now, not from when the future is first polled.
    let mut now = Box::pin(Arc::clone(&world.retry).notified_owned());
    now.as_mut().enable();
    let opened = machine.map(|machine| world.reopened(machine));
    async move {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = now => {}
            () = async {
                match opened {
                    Some(opened) => opened.await,
                    None => std::future::pending().await,
                }
            } => {}
        }
    }
}
