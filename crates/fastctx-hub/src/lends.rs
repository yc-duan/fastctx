//! Loans that end by themselves (`world-network.md`, 共享机器与借出智能体).
//! A loan past its deadline already lends nothing, on the hub and on every
//! member (`Directory::lent`); when the deadline comes the hub also takes
//! it off the list as its owner would, records that in the machine log,
//! and ends the calls and streams that were allowed under it.

use std::io;
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
        let changed = hub.lends_changed.notified();
        let (ended, next) = {
            let mut store = hub.store();
            let ended = end_due(&mut store, Timestamp::now())?;
            (ended, store.book.directory.lends.iter().filter_map(|lend| lend.until).min())
        };
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

/// Takes back the loans due at `now`, each recorded as the hub's own doing.
fn end_due(store: &mut Store, now: Timestamp) -> io::Result<bool> {
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
