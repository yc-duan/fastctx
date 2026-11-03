//! How long this machine held a thread back while the drill ran: a thread
//! that sleeps ten milliseconds over and over notes by how much it
//! overslept. Where that reached the slow poll's mark, a hub's slow polls and
//! long holds tell how loaded the machine was (a shared runner, its cores
//! taken by the drill's own hubs and machines), not what the hub did: they
//! are told, not judged, as the scale drill does where its CPU quota was
//! throttled. Blocking where a thread must not is judged whatever the load.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The longest a sleep overran, in milliseconds.
static LONGEST: AtomicU64 = AtomicU64::new(0);

/// Starts watching, for as long as the drill runs.
pub fn watch() {
    let asked = Duration::from_millis(10);
    std::thread::Builder::new()
        .name("stalls".into())
        .spawn(move || {
            loop {
                let at = Instant::now();
                std::thread::sleep(asked);
                let over = at.elapsed().saturating_sub(asked);
                LONGEST.fetch_max(u64::try_from(over.as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
            }
        })
        .expect("a thread starts");
}

/// The longest this machine held the watching thread back, where that
/// reached the slow poll's mark.
pub fn held_back() -> Option<Duration> {
    let longest = Duration::from_millis(LONGEST.load(Ordering::Relaxed));
    (longest >= fastctx_platform::task::SLOW_POLL).then_some(longest)
}
