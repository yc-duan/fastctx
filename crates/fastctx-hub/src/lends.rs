//! Loans that end by themselves (`world-network.md`, 共享机器与借出智能体).
//! A loan past its deadline already lends nothing, on the hub and on every
//! member (`Directory::lent`); when the deadline comes the hub also takes
//! it off the list as its owner would, records that in the machine log,
//! and ends the calls and streams that were allowed under it.
//!
//! A deadline is a moment of the clock, and the hub's timer counts elapsed
//! time: when the clock is set forward, the timer still waits for time that
//! the clock skipped. Every connected member's heartbeat (`wire::HEARTBEAT`)
//! therefore also looks whether the clock has reached the next deadline, so
//! what runs under a loan is cut at most one heartbeat after the clock says
//! the loan ended; nothing can run under a loan with no member connected.

use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use fastctx_net::directory::Lend;
use fastctx_types::machine_log::{Action, Actor, Outcome};
use jiff::Timestamp;

use crate::Hub;
use crate::machine_log;
use crate::store::Store;

/// Takes back each loan whose deadline has come, then waits for the next
/// deadline or for the loans to change; runs as long as the hub. A loan
/// whose deadline passed while the hub was down is taken back as it starts.
pub(crate) async fn expire(hub: &Hub) -> io::Result<()> {
    loop {
        // Nothing changes while the hub hands its records over.
        hub.moving.unfrozen().await;
        let changed = hub.lends_changed.notified();
        let (ended, next) = {
            let mut store = hub.store();
            let ended = !hub.moving.frozen() && end_due(&mut store, Timestamp::now())?;
            (ended, store.book.directory.lends.iter().filter_map(|lend| lend.until).min())
        };
        hub.next_lend_end.0.store(next.map_or(i64::MAX, |next| next.as_second()), Ordering::Relaxed);
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
/// while no loan has one.
pub(crate) struct NextEnd(pub(crate) AtomicI64);

impl Default for NextEnd {
    fn default() -> Self {
        Self(AtomicI64::new(i64::MAX))
    }
}

/// A member's heartbeat came: once the clock is in the second of the next
/// deadline, the loans are looked at again, whatever the timer waits for.
pub(crate) fn heard(hub: &Hub) {
    if Timestamp::now().as_second() >= hub.next_lend_end.0.load(Ordering::Relaxed) {
        hub.lends_changed.notify_one();
    }
}

/// Takes back the loans due at `now`, each recorded as the hub's own doing;
/// whether there were any.
pub(crate) fn end_due(store: &mut Store, now: Timestamp) -> io::Result<bool> {
    let due: Vec<Lend> = store.book.directory.lends.iter().filter(|lend| !lend.live(now)).cloned().collect();
    let hub = Actor { account: None, authenticated: true, member: None, peer: None };
    for lend in &due {
        let before = store.book.directory.clone();
        store.book.directory.lends.retain(|kept| !kept.same(lend));
        store.book.directory.settle_acting();
        let action = Action::LendEnded {
            machine: lend.machine.clone(),
            to: lend.account.clone(),
            agent: lend.agent.clone(),
            home: lend.home.clone(),
        };
        machine_log::effects(store, &before, &hub, &action).map_err(io::Error::other)?;
        machine_log::keep(store, &before, hub.clone(), action, Outcome::Done).map_err(io::Error::other)?;
    }
    Ok(!due.is_empty())
}
