//! Letters kept for a machine that has been away too long are settled as
//! not to be done (`world-network.md`, 每一笔账都有终点): a machine offline
//! more than `AWAY` gets none of what waited for it, and what waited no
//! longer grows on the hub's disk. A machine taken off the hub has its
//! letters set aside at once (`crate::store::Store::forget_letters`).

use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_types::MachineName;
use jiff::Timestamp;

use crate::Hub;

/// How long a machine may be away before what waits for it is settled.
pub(crate) const AWAY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How often the hub looks for machines away that long.
const LOOKING: Duration = Duration::from_secs(6 * 60 * 60);

/// Settles the letters of machines away too long, for as long as the hub runs.
pub(crate) async fn settle(hub: Weak<Hub>) {
    loop {
        tokio::time::sleep(Duration::from_secs(10 * 60)).await;
        let Some(hub) = hub.upgrade() else { return };
        for name in away(&hub) {
            match settle_one(&hub, &name).await {
                Ok(0) => {}
                Ok(settled) => hub.log.line(format_args!(
                    "{settled} letters for {name}, away more than 30 days, were settled as not delivered"
                )),
                Err(error) => hub.log.line(format_args!("settling the letters for {name}: {error}")),
            }
        }
        drop(hub);
        tokio::time::sleep(LOOKING.saturating_sub(Duration::from_secs(10 * 60))).await;
    }
}

/// The machines not connected that were last seen more than `AWAY` ago.
/// One whose leaving is not known (the hub stopped while it was connected)
/// is not counted away.
fn away(hub: &Hub) -> Vec<MachineName> {
    let Some(before) = since() else { return Vec::new() };
    let store = hub.store();
    store.letters.keys().filter(|name| still_away(&store, name, before)).cloned().collect()
}

/// The moment before which a machine last seen has been away too long.
fn since() -> Option<Timestamp> {
    Timestamp::now().checked_sub(jiff::SignedDuration::try_from(AWAY).ok()?).ok()
}

/// Whether `name` is not connected and was last seen before `before`: a
/// machine that came back since it was found away, and left again, was seen
/// just now, and keeps what waits for it.
fn still_away(store: &crate::store::Snapshot, name: &MachineName, before: Timestamp) -> bool {
    store
        .letters
        .get(name)
        .is_some_and(|letters| letters.connected_at.is_none() && letters.last_seen.is_some_and(|seen| seen < before))
}

/// Settles what waits for `name`: each letter is said in the log, by whom it
/// was sent, and taken away once the records number past it, as a letter
/// taken is (`crate::mailbox`).
async fn settle_one(hub: &Arc<Hub>, name: &MachineName) -> std::io::Result<usize> {
    let Some(before) = since() else { return Ok(0) };
    let Some(member) = hub.store().letters.member(name) else { return Ok(0) };
    let settled =
        member.request(|reply| crate::post::Command::Expire { before, reply }).await.map_err(std::io::Error::other)?;
    if settled > 0 {
        hub.changed_async().await;
    }
    Ok(settled)
}
