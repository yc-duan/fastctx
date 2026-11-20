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
        let next = hub
            .blocking(|hub| {
                let mut store = hub.edit_store();
                let ended = !hub.moving.frozen()
                    && end_due(&mut store, Timestamp::now()).unwrap_or_else(|error| {
                        // The deadline stands and is tried again shortly; a hub that
                        // stopped for it would end every assistance at once.
                        crate::hold_back::log(
                            &hub,
                            &ENDING,
                            format_args!("ending assistances that ran out: {error}; trying again in a minute"),
                        );
                        false
                    });
                let next = store.assist_ends.next();
                // One that could not be ended is due still: looked at again in a minute.
                let next = if next.is_some_and(|next| next <= Timestamp::now()) {
                    Timestamp::now().checked_add(jiff::SignedDuration::from_secs(60)).ok()
                } else {
                    next
                };
                drop(store);
                if ended {
                    hub.changed();
                    // Each ended for good: its machine is asked to close it.
                    crate::purges::tell(&hub, None);
                }
                next
            })
            .await;
        hub.next_assist_end.0.store(next.map_or(i64::MAX, |next| next.as_second()), Ordering::Relaxed);
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

/// When a failure to end an assistance was last told, so a lasting one is
/// told once a minute.
static ENDING: AtomicI64 = AtomicI64::new(0);

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

/// Ends the assistance due at `now`, each recorded as the hub's own doing,
/// for good: what was born under it is deleted, the purge written in the
/// same commit as the ending (`crate::purges`). One that could not be
/// written is put back as it was, and stays due. Whether there was any.
pub(crate) fn end_due(store: &mut Store, now: Timestamp) -> io::Result<bool> {
    let due = store.assist_ends.due(now);
    let hub = Actor { account: None, authenticated: true, member: None, peer: None };
    for assist in &due {
        let book = store.book.clone();
        let ended = (|| {
            let before = store.book.directory.clone();
            store.book.directory.assists.retain(|kept| !kept.same(assist));
            store.book.directory.settle_acting(&before);
            if let Some(ended) = before.assists.iter().find(|kept| kept.same(assist))
                && ended.grants().next().is_some()
            {
                crate::purges::begin(
                    store,
                    crate::purges::Ended {
                        grants: ended.grants().cloned().collect(),
                        machine: ended.machine.clone(),
                        account: ended.account.clone(),
                        agent: ended.agent.clone(),
                        home: ended.home.clone(),
                        by: None,
                    },
                )?;
            }
            let action = Action::AssistEnded {
                machine: assist.machine.clone(),
                to: assist.account.clone(),
                agent: assist.agent.clone(),
                home: assist.home.clone(),
            };
            machine_log::effects(store, &before, &hub, &action)?;
            machine_log::keep(store, &before, hub.clone(), action, Outcome::Done)
        })();
        if let Err(error) = ended {
            // Neither the ending nor its purge stands: the book is as it was
            // committed last, and written whole when it is written next.
            store.book = book;
            store.book_edit = crate::store::BookEdit::Full;
            return Err(io::Error::other(error));
        }
    }
    Ok(!due.is_empty())
}

/// Gives each assistance kept before grants were its own (`Assist::grant`),
/// at random, before any member is heard: every node then reads the grant the
/// hub keeps, which a renamed machine or an assistance given again does not
/// change or repeat (design-forget-account-v2.md F0, codex-87c8bbd9). How
/// many it gave.
pub(crate) fn grant_kept(store: &mut Store) -> io::Result<usize> {
    let mut given = 0;
    for assist in &mut store.book.directory.assists {
        if assist.grant.is_none() {
            let bytes = fastctx_net::sign::random::<8>()?;
            std::sync::Arc::make_mut(assist).grant = Some(fastctx_types::access::GrantId::from_bytes(bytes));
            given += 1;
        }
    }
    if given > 0 {
        store.save_book()?;
    }
    Ok(given)
}
