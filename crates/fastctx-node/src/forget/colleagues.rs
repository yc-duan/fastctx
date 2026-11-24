//! The colleagues a conversation asked go with it (the user, 2026-11-23:
//! 「删除一段会话，那这段会话他派发的同事也要一并删除…站在各台机器上的副本，
//! 它的一切资产，包括图片啥的也要删除…不然这些派发的同事会话，他根本就没有
//! 地方可以删」). Before a conversation that lives here goes, which colleagues
//! it asked is read and what each is left to is kept: one that lives here is
//! deleted here once the conversation is gone, as the user deleting it would
//! have it deleted, with the colleagues it asked in turn; one that lives
//! elsewhere is asked of the hub it was asked through once the conversation
//! is gone here (`Ask::ready`), and the hub has its machine delete it. What a
//! deletion kept is taken back where the conversation does not go after all,
//! and nothing of a colleague goes before the conversation has: a deletion
//! refused takes none of them with it. One here that cannot be deleted at
//! once is tried again with the hubs' asks (`sweep`), for a month. A colleague
//! is taken for one living here only by the address it was asked at, and one
//! living here is asked of no hub: from the machine a conversation lives on,
//! a hub takes a deletion as done there.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Origin;
use fastctx_types::{Address, MachineName, ResidentId};
use serde::{Deserialize, Serialize};

use super::Ask;
use crate::Node;

/// How long a colleague here is tried, from when its deletion was planned.
const TRIED_FOR_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// How long a plan waits for its conversation's deletion to begin: one
/// refused, or never come that far, is let go, and planned anew when the
/// conversation is deleted again. A plan of a node before this one is let go
/// at once.
const WAITS_FOR_MS: i64 = 10 * 60 * 1000;

/// The colleagues here one pass of the hubs' asks tries at most: what it
/// stops may take its time, and the asks wait for no colleague.
const AT_ONCE: usize = 4;

/// When this node began, in milliseconds: a plan made before was made by a
/// node that stopped before it came to an end.
static STARTED: LazyLock<i64> = LazyLock::new(|| jiff::Timestamp::now().as_millisecond());

/// Held while the colleagues to delete here are read and written, on the
/// thread that does it, so that a pass given up meanwhile writes nothing
/// over what another kept.
static FILE: Mutex<()> = Mutex::new(());

/// The colleagues being deleted here now, by a pass or by the deletion that
/// planned them, so that one is not deleted twice at once.
static TAKEN: LazyLock<Mutex<HashSet<ResidentId>>> = LazyLock::new(Default::default);

/// A colleague living here, to be deleted once the conversation that asked
/// it is gone.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Due {
    /// The conversation that asked it, as its origin names it.
    asker: Address,
    colleague: ResidentId,
    /// Planned at the end of a retention: not stopped while at work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    gentle: bool,
    /// When its deletion was planned, in milliseconds: the plan it is of.
    since: i64,
}

impl Due {
    fn pair(&self, other: &Due) -> bool {
        self.asker == other.asker && self.colleague == other.colleague
    }

    fn same(&self, other: &Due) -> bool {
        self.pair(other) && self.since == other.since
    }
}

/// Names the time this node began (`STARTED`).
pub(super) fn begin() {
    LazyLock::force(&STARTED);
}

/// What a conversation's deletion leaves its colleagues to, kept before it
/// goes.
#[derive(Default)]
pub(super) struct Plan {
    /// For those elsewhere, held back until it is gone here.
    asks: Vec<Ask>,
    /// Those of `asks` kept now.
    asks_added: Vec<Ask>,
    /// Those here, of this plan.
    here: Vec<Due>,
}

/// Reads which colleagues `id`, which lives here, asked, and keeps what each
/// is left to; `gentle` at the end of a retention.
pub(super) async fn plan(node: &Arc<Node>, id: &ResidentId, gentle: bool) -> Result<Plan, String> {
    begin();
    let acting = node.residents.about(id).map(|(about, _)| about.acting);
    let now = jiff::Timestamp::now().as_millisecond();
    let mut plan = Plan::default();
    for (colleague, account) in asked(node, id).await? {
        if names_here(node, &colleague.machine, account.as_ref()) {
            // Asked here: an id born here names no other conversation here.
            match node.residents.about(&colleague.resident) {
                Some((about, _)) => match about.origin {
                    Origin::Colleague { asked_by, .. } if asked_by.resident == *id => {
                        here(&mut plan, asked_by, colleague.resident, gentle, now);
                    }
                    _ => node.log.line(format_args!(
                        "{}, which the actions of {id} name, was not asked by it: kept",
                        colleague.resident
                    )),
                },
                None if node.residents.ever_deleted(&colleague.resident) => {}
                // Gone from here but not deleted, as moved to another machine:
                // asked of the hubs, which know where it lives now.
                None => plan.asks.extend(asks_of(node, id, &colleague, account, acting.as_ref())),
            }
            continue;
        }
        match living_here(node, id, &colleague) {
            Some(asker) => here(&mut plan, asker, colleague.resident, gentle, now),
            None => plan.asks.extend(asks_of(node, id, &colleague, account, acting.as_ref())),
        }
    }
    // Those here that name it as their asker, though its actions do not.
    for resident in node.residents.everyone() {
        if let Origin::Colleague { asked_by, .. } = &resident.about.origin
            && asked_by.resident == *id
            && here_named(node, &asked_by.machine)
        {
            here(&mut plan, asked_by.clone(), resident.id, gentle, now);
        }
    }
    if !plan.asks.is_empty() {
        plan.asks_added = super::keep(node, plan.asks.clone())
            .await
            .map_err(|error| format!("keeping what to ask the hubs of its colleagues: {error}"))?;
    }
    if !plan.here.is_empty()
        && let Err(error) = keep(node, plan.here.clone()).await
    {
        let failed = format!("keeping which of its colleagues to delete here: {error}");
        if let Err(error) = super::withdraw(node, std::mem::take(&mut plan.asks_added)).await {
            node.log.line(format_args!("taking back what to ask the hubs of {id}'s colleagues: {error}"));
        }
        return Err(failed);
    }
    Ok(plan)
}

impl Plan {
    /// Whether a colleague of it here is at work, which a retention does not
    /// stop: the colleague it is.
    pub(super) async fn at_work(&self, node: &Arc<Node>) -> Option<ResidentId> {
        for due in &self.here {
            if crate::conversation_retention::expiry::busy(node, &due.colleague).await {
                return Some(due.colleague.clone());
            }
        }
        None
    }

    /// Takes back what this kept, where `id` does not go after all; nothing
    /// kept before it, nor what another deletion of it planned since.
    pub(super) async fn take_back(self, node: &Arc<Node>, id: &ResidentId) {
        if super::going(node, id) {
            return;
        }
        if !self.asks_added.is_empty()
            && let Err(error) = super::withdraw(node, self.asks_added).await
        {
            node.log.line(format_args!("taking back what to ask the hubs of {id}'s colleagues: {error}"));
        }
        if !self.here.is_empty()
            && let Err(error) = settle(node, self.here).await
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

/// What a sweep left.
pub(super) struct Swept {
    /// Some colleague here is still to go.
    pub(super) left: bool,
    /// It deleted one, whose hubs are to be asked.
    pub(super) deleted: bool,
}

/// Deletes the colleagues here whose conversation is gone, as planned: those
/// of `only`, all of them, or of any, a few at a time.
pub(super) async fn sweep(node: &Arc<Node>, only: Option<&ResidentId>) -> Swept {
    let mut swept = Swept { left: false, deleted: false };
    let Ok(_call) = super::handing(node) else {
        swept.left = true;
        return swept;
    };
    let path = file(node);
    let kept = match fastctx_platform::task::spawn_blocking(move || {
        let _file = FILE.lock().expect("never poisoned");
        read(&path)
    })
    .await
    {
        Ok(Ok(kept)) => kept,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading which colleagues to delete here: {error}"));
            swept.left = true;
            return swept;
        }
        Err(error) => {
            node.log.line(format_args!("reading which colleagues to delete here: {error}"));
            swept.left = true;
            return swept;
        }
    };
    let now = jiff::Timestamp::now().as_millisecond();
    let (mut settled, mut tried) = (Vec::new(), 0);
    // Begun at another place each time, so that those refused again and
    // again do not keep the rest from their turn.
    let mut candidates: Vec<Due> =
        kept.into_iter().filter(|due| only.is_none_or(|id| due.asker.resident == *id)).collect();
    if !candidates.is_empty() {
        let start = (now / 1000).unsigned_abs() as usize % candidates.len();
        candidates.rotate_left(start);
    }
    for due in candidates {
        let asker = &due.asker.resident;
        let month = now.saturating_sub(due.since) > TRIED_FOR_MS;
        if !node.residents.ever_deleted(asker) {
            if super::going(node, asker) {
                swept.left = true;
            } else if due.since < *STARTED || now.saturating_sub(due.since) > WAITS_FOR_MS {
                settled.push(due);
            } else {
                swept.left = true;
            }
            continue;
        }
        match node.residents.about(&due.colleague) {
            Some((about, _)) if names(&about.origin, &due.asker) => {}
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
                if month {
                    settled.push(due);
                }
                continue;
            }
        }
        if only.is_none() && tried >= AT_ONCE {
            swept.left = true;
            continue;
        }
        let Some(_taken) = Taken::take(&due.colleague) else {
            swept.left = true;
            continue;
        };
        tried += 1;
        // At the end of a retention, deleted as one going for its own would
        // be: refused while at work, which nothing kept at the hubs stops
        // later; otherwise as the user deleting it.
        let deleted = if due.gentle {
            expiring(node, due.colleague.clone()).await
        } else {
            deleting(node, due.colleague.clone()).await.map(|_| ()).map_err(|failure| failure.message)
        };
        match deleted {
            Ok(()) => {
                node.log.line(format_args!(
                    "deleted {}, which lived here, after {asker}, the conversation that asked it",
                    due.colleague
                ));
                swept.deleted = true;
                settled.push(due);
            }
            Err(why) if month => {
                node.log.line(format_args!(
                    "giving up deleting {}, a colleague {asker} asked: tried for a month; {why}",
                    due.colleague
                ));
                settled.push(due);
            }
            Err(why) => {
                node.log.line(format_args!(
                    "deleting {}, a colleague {asker} asked, which is gone: {why}; tried again later",
                    due.colleague
                ));
                swept.left = true;
            }
        }
    }
    if !settled.is_empty()
        && let Err(error) = settle(node, settled).await
    {
        node.log.line(format_args!("keeping which colleagues are still to delete here: {error}"));
    }
    swept
}

/// A colleague being deleted here, let go when dropped.
struct Taken(ResidentId);

impl Taken {
    fn take(id: &ResidentId) -> Option<Taken> {
        TAKEN.lock().expect("never poisoned").insert(id.clone()).then(|| Taken(id.clone()))
    }
}

impl Drop for Taken {
    fn drop(&mut self) {
        TAKEN.lock().expect("never poisoned").remove(&self.0);
    }
}

/// Whether `origin` names `asker` as the conversation that asked it.
fn names(origin: &Origin, asker: &Address) -> bool {
    matches!(origin, Origin::Colleague { asked_by, .. } if asked_by == asker)
}

/// Adds `colleague`, which `asker` asked, to those to delete here.
fn here(plan: &mut Plan, asker: Address, colleague: ResidentId, gentle: bool, now: i64) {
    if !plan.here.iter().any(|due| due.colleague == colleague) {
        plan.here.push(Due { asker, colleague, gentle, since: now });
    }
}

/// The colleague recorded at `colleague`, when it lives here: one of that id
/// named by `id` as its asker, as asked here, or as asked where both lived
/// before they came here together. What it names its asker by.
fn living_here(node: &Node, id: &ResidentId, colleague: &Address) -> Option<Address> {
    let (about, _) = node.residents.about(&colleague.resident)?;
    match about.origin {
        Origin::Colleague { asked_by, .. }
            if asked_by.resident == *id
                && (here_named(node, &asked_by.machine) || asked_by.machine == colleague.machine) =>
        {
            Some(asked_by)
        }
        _ => None,
    }
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
/// account it was asked for where one is recorded. A record without where
/// and whom it asked is passed over and told in the log; actions that cannot
/// be read at all keep it from going this time. Of one whose folder is gone,
/// its actions are not read: reading them would record them anew.
async fn asked(node: &Arc<Node>, id: &ResidentId) -> Result<BTreeMap<Address, Option<HubAccount>>, String> {
    let folder = node.world.conversation(id).path().to_path_buf();
    let path = node.world.conversation(id).actions();
    let reading = move || -> std::io::Result<(Vec<(MachineName, String)>, usize)> {
        if !fastctx_platform::disk::exists(&folder)? {
            return Ok((Vec::new(), 0));
        }
        crate::residents::activity::asked_readable(&path)
    };
    let (asked, unread) = fastctx_platform::task::spawn_blocking(reading)
        .await
        .map_err(|error| format!("reading the colleagues {id} asked: {error}"))?
        .map_err(|error| format!("reading the colleagues {id} asked: {error}"))?;
    if unread > 0 {
        node.log.line(format_args!("{unread} of the colleagues {id} asked could not be read from its actions"));
    }
    let mut found = BTreeMap::new();
    for (machine, result) in asked {
        let Ok(resident) = result.parse::<ResidentId>() else { continue };
        let colleague = Address { machine, resident };
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

/// `elsewhere`, told in the log where no hub is to be asked.
fn asks_of(
    node: &Node,
    asker: &ResidentId,
    colleague: &Address,
    account: Option<HubAccount>,
    acting: Option<&Acting>,
) -> Vec<Ask> {
    let asks = elsewhere(node, asker, colleague, account, acting);
    if asks.is_empty() {
        node.log.line(format_args!(
            "{} on {}, a colleague {asker} asked, is asked of no hub to be deleted: none this computer is on has its machine",
            colleague.resident, colleague.machine
        ));
    }
    asks
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

/// `super::expired` of a colleague, its future boxed: refused while it is at
/// work, asked once nothing new can begin in it (`idle`).
fn expiring(node: &Arc<Node>, id: ResidentId) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
    Box::pin(async move { super::expired(node, &id, idle()).await })
}

/// What a colleague going at the end of its conversation's retention is
/// asked once nothing new can begin in it: that nothing of it is at work.
fn idle() -> crate::data::delete::Still {
    Arc::new(|node: Arc<Node>, id: ResidentId| {
        Box::pin(async move {
            if crate::conversation_retention::expiry::busy(&node, &id).await {
                return Err("it is at work".into());
            }
            Ok(())
        })
    })
}

fn file(node: &Node) -> PathBuf {
    node.world.path().join("forget-colleagues.json")
}

/// Keeps `due`, each in place of one kept for the same colleague of the same
/// asker: the newest plan of a colleague is the one it goes by.
async fn keep(node: &Node, due: Vec<Due>) -> std::io::Result<()> {
    let path = file(node);
    fastctx_platform::task::spawn_blocking(move || {
        let _file = FILE.lock().expect("never poisoned");
        let mut kept = read(&path)?;
        let mut changed = false;
        for one in due {
            match kept.iter().position(|kept| kept.pair(&one)) {
                Some(at) if kept[at].same(&one) => {}
                Some(at) => {
                    kept[at] = one;
                    changed = true;
                }
                None => {
                    kept.push(one);
                    changed = true;
                }
            }
        }
        if changed { write(&path, &kept) } else { Ok(()) }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Lets go of `settled`, each only as that plan kept it.
async fn settle(node: &Node, settled: Vec<Due>) -> std::io::Result<()> {
    let path = file(node);
    fastctx_platform::task::spawn_blocking(move || {
        let _file = FILE.lock().expect("never poisoned");
        let mut kept = read(&path)?;
        let before = kept.len();
        kept.retain(|one| !settled.iter().any(|settled| settled.same(one)));
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
