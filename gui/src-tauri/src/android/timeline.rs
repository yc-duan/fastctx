//! A conversation page on Android reads its timeline as a desktop page
//! does, a page at a time, never the whole history (`timeline_watch`). What
//! differs is the phone's: a hidden page lets its live stream go, and shown
//! again it is given the window it holds and caught up from there; and an
//! opening that failed while the connection was being replaced, as on a
//! foreground resume, is tried again on the new one.

use std::time::Duration;

use fastctx_client::{Watching, World};
use fastctx_net::link::Status;
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use tokio::sync::watch;

use crate::timeline_watch::{self, Opened, Readers, Shared};
use crate::watch_delivery::Sender;
use crate::{NotWatched, Piece};

/// How many times an opening is tried again for the connection it ran on.
const TRIES: u32 = 3;
/// How long an opening that failed while connecting waits for the
/// connection to settle before it is tried again.
const SETTLE: Duration = Duration::from_secs(15);

/// Opens the timeline as `timeline_watch::open` does; `None` for an older
/// node, read the whole-history way (`super::watching`).
pub(crate) async fn open(world: &World, resident: ResidentId, machine: Option<Written>) -> Result<Option<Opened>, NotWatched> {
    let (_, mut connection) =
        world.device().expect("Android device").machine_changes(machine.as_ref()).map_err(|error| error.to_string())?;
    let mut tries = 0;
    loop {
        let state = connection.borrow_and_update().clone();
        if let Status::Stopped { reason } = state {
            return Err(reason.into());
        }
        let opened = timeline_watch::open(world, resident.clone(), machine.clone()).await;
        let passing = matches!(&opened, Err(error) if !error.deleted && !error.refused);
        if !passing || tries >= TRIES {
            return opened;
        }
        tries += 1;
        if connection.has_changed().unwrap_or(false) {
            continue;
        }
        if matches!(state, Status::Connecting { .. })
            && matches!(tokio::time::timeout(SETTLE, connection.changed()).await, Ok(Ok(())))
        {
            continue;
        }
        return opened;
    }
}

/// A page that cannot be live now, hidden.
fn paused() -> Piece {
    Piece::Interrupted { error: None, since: crate::epoch_ms(std::time::SystemTime::now()) }
}

/// Until the page is hidden; never, once nothing tells.
async fn hidden(visible: &mut watch::Receiver<bool>) {
    loop {
        if visible.changed().await.is_err() {
            return std::future::pending().await;
        }
        if !*visible.borrow_and_update() {
            return;
        }
    }
}

/// Follows the reader as `Readers::run` does while the page is shown; a
/// hidden page holds no live stream and nothing queued for it.
pub(crate) async fn run(readers: Readers, id: u64, reader: Shared, mut initial: Option<Watching>, pieces: Sender<Piece>) {
    let mut visible = super::visibility();
    loop {
        if !*visible.borrow_and_update() {
            drop(initial.take());
            if pieces.send(paused()).is_err() {
                return;
            }
            loop {
                tokio::select! {
                    () = reader.closed() => return,
                    changed = visible.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        if *visible.borrow_and_update() {
                            break;
                        }
                    }
                }
            }
        }
        // Shown again, with no watch in hand, the page is given the window
        // the reader holds and the watch catches up from where it read.
        tokio::select! {
            () = readers.run(id, reader.clone(), initial.take(), &pieces) => return,
            () = hidden(&mut visible) => {}
        }
    }
}
