//! Telling members that the directory changed, or that who is online did.
//!
//! A member whose session reads what changes in its view is sent just that
//! (`Hub::push_views`), and who is online as it changes (`Down::Presence`).
//! One that reads neither asks for its whole view again each time it is told
//! (`Down::DirectoryChanged`). Told of every machine that came or went, such
//! members read their whole views every second or two, and the hub's line
//! went on that before anything else. So each such session is told at most
//! once each `OLD_DIRECTORY_SPACING` of a change that narrows nothing it may
//! do (`Hub::configuration_changed`), and at most once each
//! `OLD_PRESENCE_SPACING` of who is online alone, what came meanwhile told
//! together when that has passed. A change of what members may do
//! (`Hub::changed`, `Hub::relations_changed`) is told to every session at
//! once, as before; what lost its standing is cut by `Hub::changed` itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use fastctx_net::wire::Down;
use fastctx_types::MachineName;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::{Hub, Out, outq};

/// How long the hub waits after telling members before it tells them again.
const DIRECTORY_SPACING: Duration = Duration::from_secs(1);
/// How often a session that reads neither changes to its view nor who is
/// online is told of a change that narrows nothing it may do.
const OLD_DIRECTORY_SPACING: Duration = Duration::from_secs(5);
/// How often such a session is told of who is online alone, which it shows
/// and nothing it may do waits on.
const OLD_PRESENCE_SPACING: Duration = Duration::from_secs(30);

/// A member's session: its name and generation.
type Session = (MachineName, u64);

/// What the directory changes since the members were last told touched: a
/// change that alters what anyone may see or do touches all of them; one to
/// an account's or a machine's own settings (`Hub::configuration_changed_for`)
/// touches the views that show that account or machine, and no others, so
/// only theirs are made again (codex-12f4ed09).
#[derive(Default)]
pub(crate) struct Scope {
    pub(crate) all: bool,
    pub(crate) accounts: std::collections::BTreeSet<fastctx_types::AccountName>,
    pub(crate) machines: std::collections::BTreeSet<MachineName>,
}

/// When each session that reads its whole view was told, and when one owed
/// a change is to be told it.
#[derive(Default)]
struct Spacing {
    told: HashMap<Session, Instant>,
    owed: HashMap<Session, Instant>,
}

/// Tells members, as the directory and who is online change, while the hub
/// runs. `authority` is set as what members may do changes.
pub(crate) async fn run(hub: Weak<Hub>, changed: Arc<Notify>, present: Arc<Notify>, authority: Arc<AtomicBool>) {
    let mut spacing = Spacing::default();
    loop {
        let due = spacing.owed.values().min().copied();
        let woke = tokio::select! {
            () = changed.notified() => Some(true),
            () = present.notified() => Some(false),
            () = until(due) => None,
        };
        // Taken only when woken by a change of the directory: one of who is
        // online leaves it for that change, which wakes this next
        // (codex-580b4d87).
        let urgent = woke == Some(true) && authority.swap(false, Ordering::SeqCst);
        let Some(hub) = hub.upgrade() else { return };
        spacing = hub.blocking(move |hub| tell(&hub, woke, urgent, spacing)).await;
        drop(hub);
        tokio::time::sleep(DIRECTORY_SPACING).await;
    }
}

/// Resolves at `due`, or never.
async fn until(due: Option<Instant>) {
    match due {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Tells the sessions what `woke` says changed (`Some(true)` the directory,
/// `Some(false)` who is online, `None` nothing new), and those owed a change
/// whose time came.
fn tell(hub: &Hub, woke: Option<bool>, urgent: bool, mut spacing: Spacing) -> Spacing {
    let now = Instant::now();
    let mut outs: Vec<outq::Outq> = Vec::new();
    let mut changes = Vec::new();
    // What the changes told now touched; none recorded counts as all.
    let scope = match woke {
        Some(true) => hub.take_directory_scope().filter(|scope| !scope.all),
        _ => None,
    };
    {
        let sessions = hub.sessions();
        for (name, session) in sessions.iter() {
            let id = (name.clone(), session.generation);
            if let Some(every) = woke {
                if every && session.view_deltas {
                    if scope.as_ref().is_none_or(|scope| session.out.view_shows(&scope.accounts, &scope.machines)) {
                        changes.push((name.clone(), session.key, session.generation, session.out.clone()));
                    }
                    continue;
                }
                if every || !session.presence {
                    let wait = if every { OLD_DIRECTORY_SPACING } else { OLD_PRESENCE_SPACING };
                    match spacing.told.get(&id) {
                        Some(at) if !urgent && now.duration_since(*at) < wait => {
                            let due = *at + wait;
                            spacing.owed.entry(id).and_modify(|owed| *owed = (*owed).min(due)).or_insert(due);
                        }
                        _ => {
                            outs.push(session.out.clone());
                            spacing.owed.remove(&id);
                            spacing.told.insert(id, now);
                        }
                    }
                    continue;
                }
            }
            if spacing.owed.get(&id).is_some_and(|due| *due <= now) {
                outs.push(session.out.clone());
                spacing.owed.remove(&id);
                spacing.told.insert(id, now);
            }
        }
        let open =
            |(name, generation): &Session| sessions.get(name).is_some_and(|session| session.generation == *generation);
        spacing.told.retain(|id, _| open(id));
        spacing.owed.retain(|id, _| open(id));
    }
    for out in outs {
        let _ = out.send(Out::Frame(Down::DirectoryChanged));
    }
    if !changes.is_empty() {
        hub.push_views(changes);
    }
    spacing
}
