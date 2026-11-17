//! A conversation page on Android reads its timeline as a desktop page does
//! (`crate::replica`). What differs is the phone's: an opening that failed
//! while the connection was being replaced, as on a foreground resume, is
//! tried again on the new one; and a hidden page lets its live watch go,
//! and shown again reads anew from what the timeline is then.

use std::time::Duration;

use fastctx_client::World;
use fastctx_net::link::Status;
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use tokio::sync::watch;

use crate::replica::{self, Opened};
use crate::watch_delivery::Sender;
use crate::{NotWatched, Piece};

/// How many times an opening is tried again for the connection it ran on.
const TRIES: u32 = 3;
/// How long an opening that failed while connecting waits for the
/// connection to settle before it is tried again.
const SETTLE: Duration = Duration::from_secs(15);

/// Opens the timeline as `replica::open` does; `None` for an older node,
/// read the whole-history way (`super::watching`).
pub(crate) async fn open(
    world: &World,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<Option<Opened>, NotWatched> {
    let (_, mut connection) =
        world.device().expect("Android device").machine_changes(machine.as_ref()).map_err(|error| error.to_string())?;
    let mut tries = 0;
    loop {
        let state = connection.borrow_and_update().clone();
        if let Status::Stopped { reason } = state {
            return Err(reason.into());
        }
        let opened = replica::open(world, resident.clone(), machine.clone()).await;
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

/// Follows the conversation as `replica::follow` does while the page is
/// shown; a hidden page holds no watch, and shown again it reads anew.
pub(crate) async fn follow(opened: &Opened, again: bool, pieces: &Sender<Piece>) {
    let mut visible = super::visibility();
    let mut again = again;
    loop {
        if !*visible.borrow_and_update() {
            if pieces.send(paused()).is_err() {
                return;
            }
            loop {
                if visible.changed().await.is_err() {
                    return;
                }
                if *visible.borrow_and_update() {
                    break;
                }
            }
            again = true;
        }
        tokio::select! {
            () = replica::follow(opened, again, pieces) => return,
            () = hidden(&mut visible) => {}
        }
    }
}
