//! The colleagues a conversation asked go with it (the user, 2026-11-23:
//! 「删除一段会话，那这段会话他派发的同事也要一并删除…站在各台机器上的副本，
//! 它的一切资产，包括图片啥的也要删除…不然这些派发的同事会话，他根本就没有
//! 地方可以删」). Before a conversation that lives here goes, which colleagues
//! it asked is read and what each is left to is kept: one that lives here is
//! deleted here once the conversation is gone, as the user deleting it would
//! have it deleted, with the colleagues it asked in turn; one that lives
//! elsewhere is asked of the hub it was asked through once the conversation
//! is gone here (`Ask::ready`), and the hub has its machine delete it. What is
//! kept is taken back where the conversation does not go after all, and
//! nothing of a colleague goes before the conversation has: a deletion
//! refused takes none of them with it. One here that cannot be deleted at
//! once is tried again with the hubs' asks (`sweep`), for a month. One that
//! lives here is asked of no hub, whatever name its machine was asked by:
//! from the machine it lives on, a hub takes a deletion as done there.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Action, Origin};
use fastctx_types::{Address, MachineName, ResidentId};
use serde::{Deserialize, Serialize};

use super::Ask;
use crate::Node;

/// How long a colleague here is tried, from when its deletion was planned.
const TRIED_FOR_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// How long a plan waits for its conversation to go: one whose deletion was
/// refused, or never came that far, is let go, and planned anew when it is
/// deleted again.
const WAITS_FOR_MS: i64 = 24 * 60 * 60 * 1000;

/// A colleague living here, to be deleted once the conversation that asked
/// it is gone.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Due {
    asker: ResidentId,
    colleague: ResidentId,
    /// When its deletion was planned, in milliseconds.
    since: i64,
}

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        (&self.asker, &self.colleague) == (&other.asker, &other.colleague)
    }
}

/// What a conversation's deletion leaves its colleagues to, kept before it
/// goes.
#[derive(Default)]
pub(super) struct Plan {
    /// For those elsewhere, held back until it is gone here.
    asks: Vec<Ask>,
    /// Those of `asks` kept now.
    asks_added: Vec<Ask>,
    /// Those here.
    here: Vec<Due>,
    /// Those of `here` kept now.
    here_added: Vec<Due>,
}

/// Reads which colleagues `id`, which lives here, asked, and keeps what each
/// is left to.
pub(super) async fn plan(node: &Arc<Node>, id: &ResidentId) -> Result<Plan, String> {
    let acting = node.residents.about(id).map(|(about, _)| about.acting);
    let now = jiff::Timestamp::now().as_millisecond();
    let mut plan = Plan::default();
    for (colleague, account) in asked(node, id).await? {
        match node.residents.about(&colleague.resident) {
            // It lives here, whatever name its machine was asked by then.
            Some((about, _)) if named_by(&about.origin, id) => here(&mut plan, id, colleague.resident, now),
            Some(_) => node.log.line(format_args!(
                "{}, which the actions of {id} name, was not asked by it: kept",
                colleague.resident
            )),
            None if node.residents.ever_deleted(&colleague.resident)
                || names_here(node, &colleague.machine, account.as_ref()) => {}
            None => {
                let asks = elsewhere(node, id, &colleague, account, acting.as_ref());
                if asks.is_empty() {
                    node.log.line(format_args!(
                        "{} on {}, a colleague {id} asked, is asked of no hub to be deleted: none this computer is on has its machine",
                        colleague.resident, colleague.machine
                    ));
                }
                plan.asks.extend(asks);
            }
        }
    }
    // Those here that name it as their asker, though its actions do not.
    for resident in node.residents.everyone() {
        if let Origin::Colleague { asked_by, .. } = &resident.about.origin
            && asked_by.resident == *id
            && here_named(node, &asked_by.machine)
        {
            here(&mut plan, id, resident.id, now);
        }
    }
    if !plan.asks.is_empty() {
        plan.asks_added = super::keep(node, plan.asks.clone())
            .await
            .map_err(|error| format!("keeping what to ask the hubs of its colleagues: {error}"))?;
    }
    if !plan.here.is_empty() {
        match keep(node, plan.here.clone()).await {
            Ok(added) => plan.here_added = added,
            Err(error) => {
                let failed = format!("keeping which of its colleagues to delete here: {error}");
                if let Err(error) = super::withdraw(node, std::mem::take(&mut plan.asks_added)).await {
                    node.log.line(format_args!("taking back what to ask the hubs of {id}'s colleagues: {error}"));
                }
                return Err(failed);
            }
        }
    }
    Ok(plan)
}

impl Plan {
    /// Takes back what this kept, where `id` does not go after all; nothing
    /// kept before it.
    pub(super) async fn take_back(self, node: &Arc<Node>, id: &ResidentId) {
        if super::going(node, id) {
            return;
        }
        if !self.asks_added.is_empty()
            && let Err(error) = super::withdraw(node, self.asks_added).await
        {
            node.log.line(format_args!("taking back what to ask the hubs of {id}'s colleagues: {error}"));
        }
        if !self.here_added.is_empty()
            && let Err(error) = settle(node, self.here_added).await
        {
            node.log.line(format_args!("taking back which colleagues of {id} to delete here: {error}"));
        }
    }

    /// `id` is gone here: what this kept is kept again, as a deletion of it
    /// refused meanwhile may have taken back what the two shared, and the
    /// colleagues here go now; the hubs are asked of the others.
    pub(super) async fn follow(self, node: &Arc<Node>, id: &ResidentId) {
        let asking = !self.asks.is_empty() || !self.here.is_empty();
        if !self.asks.is_empty()
            && let Err(error) = super::keep(node, self.asks).await
        {
            node.log.line(format_args!("keeping what to ask the hubs of {id}'s colleagues: {error}"));
        }
        if !self.here.is_empty() {
            if let Err(error) = keep(node, self.here).await {
                node.log.line(format_args!("keeping which colleagues of {id} to delete here: {error}"));
            }
            sweep(node, Some(id)).await;
        }
        if asking {
            super::wake(node);
        }
    }
}

/// Deletes the colleagues here whose conversation is gone, as planned: those
/// of `only`, or of any. Says whether any is still to go.
pub(super) async fn sweep(node: &Arc<Node>, only: Option<&ResidentId>) -> bool {
    let Ok(_call) = super::handing(node) else { return true };
    let kept = {
        let _file = node.forget_asks.file.lock().await;
        let path = file(node);
        match fastctx_platform::task::spawn_blocking(move || read(&path)).await {
            Ok(Ok(kept)) => kept,
            Ok(Err(error)) => {
                node.log.line(format_args!("reading which colleagues to delete here: {error}"));
                return true;
            }
            Err(error) => {
                node.log.line(format_args!("reading which colleagues to delete here: {error}"));
                return true;
            }
        }
    };
    let now = jiff::Timestamp::now().as_millisecond();
    let (mut settled, mut left) = (Vec::new(), false);
    for due in kept.into_iter().filter(|due| only.is_none_or(|id| due.asker == *id)) {
        if !node.residents.ever_deleted(&due.asker) {
            if super::going(node, &due.asker) || now.saturating_sub(due.since) < WAITS_FOR_MS {
                left = true;
            } else {
                settled.push(due);
            }
            continue;
        }
        match node.residents.about(&due.colleague) {
            Some((about, _)) if named_by(&about.origin, &due.asker) => {}
            // Another's now, or gone already.
            Some(_) => {
                settled.push(due);
                continue;
            }
            None if node.residents.ever_deleted(&due.colleague) => {
                settled.push(due);
                continue;
            }
            // Not known here and not known deleted: kept for its month, as
            // nothing here says it went.
            None => {
                if now.saturating_sub(due.since) > TRIED_FOR_MS {
                    settled.push(due);
                }
                continue;
            }
        }
        match deleting(node, due.colleague.clone()).await {
            Ok(_) => {
                node.log.line(format_args!(
                    "deleted {}, which lived here, after {}, the conversation that asked it",
                    due.colleague, due.asker
                ));
                settled.push(due);
            }
            Err(failure) if now.saturating_sub(due.since) > TRIED_FOR_MS => {
                node.log.line(format_args!(
                    "giving up deleting {}, a colleague {} asked: tried for a month; {}",
                    due.colleague, due.asker, failure.message
                ));
                settled.push(due);
            }
            Err(failure) => {
                node.log.line(format_args!(
                    "deleting {}, a colleague {} asked, which is gone: {}; tried again later",
                    due.colleague, due.asker, failure.message
                ));
                left = true;
            }
        }
    }
    if !settled.is_empty()
        && let Err(error) = settle(node, settled).await
    {
        node.log.line(format_args!("keeping which colleagues are still to delete here: {error}"));
    }
    left
}

/// Adds the colleague `colleague` of `asker` to those to delete here.
fn here(plan: &mut Plan, asker: &ResidentId, colleague: ResidentId, now: i64) {
    if !plan.here.iter().any(|due| due.colleague == colleague) {
        plan.here.push(Due { asker: asker.clone(), colleague, since: now });
    }
}

/// Whether `origin` says the conversation `asker` asked it.
fn named_by(origin: &Origin, asker: &ResidentId) -> bool {
    matches!(origin, Origin::Colleague { asked_by, .. } if asked_by.resident == *asker)
}

/// Whether `machine`, as a colleague was asked on it, names this computer:
/// by the hub it was asked through, or else as `here_named` has it.
fn names_here(node: &Node, machine: &MachineName, account: Option<&HubAccount>) -> bool {
    match account.and_then(|account| node.link_of(&account.hub)) {
        Some(link) => link.view().is_me(machine),
        None => here_named(node, machine),
    }
}

/// Whether `machine` names this computer and no other: a name it had, which
/// a hub it is on now gives another machine, names that one.
fn here_named(node: &Node, machine: &MachineName) -> bool {
    node.is_me(machine)
        && !node.links().iter().any(|link| {
            let view = link.view();
            view.member(machine).is_some() && !view.is_me(machine)
        })
}

/// The colleagues `id` asked, as its actions recorded them, each with the
/// account it was asked for where one is recorded. A record that cannot be
/// read is passed over and told in the log; actions that cannot be read at
/// all keep it from going this time. Of one whose folder is gone, its
/// actions are not read: reading them would record them anew.
async fn asked(node: &Arc<Node>, id: &ResidentId) -> Result<BTreeMap<Address, Option<HubAccount>>, String> {
    let folder = node.world.conversation(id).path().to_path_buf();
    let path = node.world.conversation(id).actions();
    let reading = move || -> std::io::Result<(Vec<Action>, usize)> {
        if !fastctx_platform::disk::exists(&folder)? {
            return Ok((Vec::new(), 0));
        }
        crate::residents::activity::asked_readable(&path)
    };
    let (actions, unread) = fastctx_platform::task::spawn_blocking(reading)
        .await
        .map_err(|error| format!("reading the colleagues {id} asked: {error}"))?
        .map_err(|error| format!("reading the colleagues {id} asked: {error}"))?;
    if unread > 0 {
        node.log.line(format_args!("{unread} of the colleagues {id} asked could not be read from its actions"));
    }
    let mut found = BTreeMap::new();
    for action in actions {
        let Ok(resident) = action.result.parse::<ResidentId>() else { continue };
        let colleague = Address { machine: action.machine, resident };
        let account = match crate::collab::asked_account(node, id, &colleague).await {
            Ok(account) => account,
            Err(failure) => {
                node.log.line(format_args!(
                    "reading the account {id} asked {} for: {}",
                    colleague.resident, failure.message
                ));
                None
            }
        };
        found.entry(colleague).or_insert(account);
    }
    Ok(found)
}

/// What to ask of the hubs for `colleague`, which lives elsewhere: of the hub
/// it was asked through, as the account it was asked for; for one asked
/// before that was recorded, of each hub its machine is on, as each account
/// the conversation that asked it works for there. Each is held back until
/// `asker` is gone here.
fn elsewhere(
    node: &Node,
    asker: &ResidentId,
    colleague: &Address,
    account: Option<HubAccount>,
    acting: Option<&Acting>,
) -> Vec<Ask> {
    let ask = |hub, account| Ask {
        hub,
        account,
        conversation: colleague.clone(),
        after: Some(asker.clone()),
        round: None,
        since: None,
    };
    if let Some(HubAccount { hub, account }) = account {
        return vec![ask(hub, account)];
    }
    let Some(acting) = acting else { return Vec::new() };
    node.links()
        .iter()
        .filter(|link| link.view().member(&colleague.machine).is_some())
        .flat_map(|link| {
            let hub = link.membership().id.clone();
            node.accounts_on(link, acting).into_iter().map(move |account| (hub.clone(), account))
        })
        .map(|(hub, account)| ask(hub, account))
        .collect()
}

/// `super::delete_here`, its future boxed: deleting a colleague deletes the
/// colleagues it asked in turn.
fn deleting(node: &Arc<Node>, id: ResidentId) -> Pin<Box<dyn Future<Output = Result<bool, Failure>> + Send + '_>> {
    Box::pin(async move { super::delete_here(node, &id).await })
}

fn file(node: &Node) -> PathBuf {
    node.world.path().join("forget-colleagues.json")
}

/// Keeps `due` beside what is kept already; says which it added.
async fn keep(node: &Arc<Node>, due: Vec<Due>) -> std::io::Result<Vec<Due>> {
    let _file = node.forget_asks.file.lock().await;
    let path = file(node);
    fastctx_platform::task::spawn_blocking(move || {
        let mut kept = read(&path)?;
        let mut added = Vec::new();
        for one in due {
            if !kept.contains(&one) {
                kept.push(one.clone());
                added.push(one);
            }
        }
        if !added.is_empty() {
            write(&path, &kept)?;
        }
        Ok(added)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Lets go of `settled`.
async fn settle(node: &Arc<Node>, settled: Vec<Due>) -> std::io::Result<()> {
    let _file = node.forget_asks.file.lock().await;
    let path = file(node);
    fastctx_platform::task::spawn_blocking(move || {
        let mut kept = read(&path)?;
        let before = kept.len();
        kept.retain(|one| !settled.contains(one));
        if kept.len() == before {
            return Ok(());
        }
        write(&path, &kept)
    })
    .await
    .map_err(std::io::Error::other)?
}

fn read(path: &Path) -> std::io::Result<Vec<Due>> {
    match fastctx_platform::disk::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write(path: &Path, due: &[Due]) -> std::io::Result<()> {
    if due.is_empty() {
        return match fastctx_platform::disk::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(due).map_err(std::io::Error::other)?;
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)
}
