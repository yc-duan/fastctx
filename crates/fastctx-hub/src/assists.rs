//! Assistance that ends by itself (`world-network.md`, 共享机器与智能体协助).
//! An assistance past its deadline already gives nothing, on the hub and on
//! every member (`Directory::assisting`); when the deadline comes the hub
//! also takes it off the list as its owner would, records that in the
//! machine log, and ends the calls and streams that were allowed under it.
//!
//! A deadline is a moment of the clock, and the hub's timer counts elapsed
//! time: when the clock is set forward, the timer still waits for time that
//! the clock skipped. Every connected member's heartbeat (`wire::HEARTBEAT`)
//! therefore also looks whether the clock has reached the next deadline, so
//! what runs under an assistance is cut at most one heartbeat after the
//! clock says it ended; nothing can run under one with no member connected.

use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use fastctx_net::directory::Assist;
use fastctx_types::machine_log::{Action, Actor, Outcome};
use jiff::Timestamp;

use crate::Hub;
use crate::machine_log;
use crate::store::Store;

/// Ends each assistance whose deadline has come, then waits for the next
/// deadline or for the assistance to change; runs as long as the hub. One
/// whose deadline passed while the hub was down is ended as it starts.
pub(crate) async fn expire(hub: &Hub) -> io::Result<()> {
    loop {
        // Nothing changes while the hub hands its records over.
        hub.moving.unfrozen().await;
        let changed = hub.assists_changed.notified();
        let (ended, next) = {
            let mut store = hub.store();
            let ended = !hub.moving.frozen() && end_due(&mut store, Timestamp::now())?;
            (ended, store.book.directory.assists.iter().filter_map(|assist| assist.until).min())
        };
        hub.next_assist_end.0.store(next.map_or(i64::MAX, |next| next.as_second()), Ordering::Relaxed);
        if ended {
            hub.changed();
        }
        match next {
            Some(next) => {
                let wait = next.duration_since(Timestamp::now()).as_secs_f64().max(0.0);
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs_f64(wait)) => {},
                    () = changed => {},
                }
            }
            None => changed.await,
        }
    }
}

/// The second of the clock in which the next deadline falls; `i64::MAX`
/// while no assistance has one.
pub(crate) struct NextEnd(pub(crate) AtomicI64);

impl Default for NextEnd {
    fn default() -> Self {
        Self(AtomicI64::new(i64::MAX))
    }
}

/// A member's heartbeat came: once the clock is in the second of the next
/// deadline, the assistance is looked at again, whatever the timer waits
/// for.
pub(crate) fn heard(hub: &Hub) {
    if Timestamp::now().as_second() >= hub.next_assist_end.0.load(Ordering::Relaxed) {
        hub.assists_changed.notify_one();
    }
}

/// Ends the assistance due at `now`, each recorded as the hub's own doing;
/// whether there was any.
pub(crate) fn end_due(store: &mut Store, now: Timestamp) -> io::Result<bool> {
    let due: Vec<std::sync::Arc<Assist>> =
        store.book.directory.assists.iter().filter(|assist| !assist.live(now)).cloned().collect();
    let hub = Actor { account: None, authenticated: true, member: None, peer: None };
    for assist in &due {
        let before = store.book.directory.clone();
        store.book.directory.assists.retain(|kept| !kept.same(assist));
        store.book.directory.settle_acting();
        let action = Action::AssistEnded {
            machine: assist.machine.clone(),
            to: assist.account.clone(),
            agent: assist.agent.clone(),
            home: assist.home.clone(),
        };
        machine_log::effects(store, &before, &hub, &action).map_err(io::Error::other)?;
        machine_log::keep(store, &before, hub.clone(), action, Outcome::Done).map_err(io::Error::other)?;
    }
    Ok(!due.is_empty())
}
