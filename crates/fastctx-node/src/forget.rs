//! Deleting a conversation from every machine and device keeping something
//! of it (`world-colleagues.md`, 用户在任何地方和居民说话).
//!
//! The user asks from any computer or phone. A conversation that lives here
//! is deleted here at once, and each hub it works for an account of is then
//! asked to have its other members delete what they keep. One that lives
//! elsewhere is asked of its hub, which has its machine delete it first. A
//! hub this computer is not connected to now is asked once it is: what is
//! still to be asked is kept in `forget-asks.json` beside the World.
//!
//! Told by a hub (`Down::Forget`), this machine deletes what it keeps: of a
//! conversation that lives here, all of it, once it is sure the conversation
//! works for the account the deletion is for; of one that lives elsewhere,
//! the guest rooms it had here, its commands there stopped first. Either
//! way, the copy kept here from showing it goes too.

use std::path::PathBuf;
use std::sync::Arc;

use fastctx_net::wire::{Forgetting, Forgot};
use fastctx_types::access::{Acting, HubId};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::Here as _;
use fastctx_types::management::Forgetting as How;
use fastctx_types::{AccountName, Address, MachineName, ResidentId};
use serde::{Deserialize, Serialize};

use crate::Node;
use crate::data::delete::Stop;

/// What is still to be asked of hubs, and what wakes the asking.
#[derive(Default)]
pub(crate) struct Asks {
    /// Held while the kept asks are read and written.
    file: crate::ranked::Forgetting,
    wake: Arc<tokio::sync::Notify>,
}

/// A deletion still to be asked of a hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Ask {
    hub: HubId,
    account: AccountName,
    conversation: Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<ResidentId>,
}

/// What whoever asks about `resident`, which lives here no more, is told:
/// that it was deleted, which asking again will not change.
pub(crate) fn gone(node: &Node, resident: &ResidentId) -> Failure {
    Failure::deleted(match node.residents.deleted(resident) {
        Some(at) => format!("{resident} was deleted from {} at {at}", node.machine()),
        None => format!("no conversation {resident} is on {}", node.machine()),
    })
}

/// The user asks, on this computer, that the conversation of `resident` on
/// `machine` be deleted from every machine and device keeping something of
/// it.
pub(crate) async fn ask(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: fastctx_types::access::Written,
) -> Result<How, Failure> {
    let _call = handing(node)?;
    if node.is_here(&machine) {
        // Whom it works for is read before its records go.
        let asks: Vec<Ask> = match node.residents.about(&resident) {
            Some((about, _)) => pairs(node, &about.acting)
                .into_iter()
                .map(|(hub, account)| Ask {
                    hub,
                    account,
                    conversation: Address { machine: node.machine(), resident: resident.clone() },
                    after: None,
                })
                .collect(),
            None => Vec::new(),
        };
        home(node, &resident).await.map_err(Failure::new)?;
        node.log.line(format_args!("deleted {resident}, which lived here, as the user asked"));
        if asks.is_empty() {
            return Ok(How::Deleted);
        }
        keep(node, asks).await.map_err(|error| Failure::new(format!("keeping what to ask the hubs: {error}")))?;
        return Ok(if send_kept(node).await { How::Asked } else { How::Queued });
    }
    let link = node
        .locate(&machine)?
        .into_iter()
        .next()
        .ok_or_else(|| Failure::new(format!("no hub this computer is on has a machine named {machine}")))?;
    let account = link.account_of(&machine.name, &resident).map_err(Failure::new)?;
    let conversation = Address { machine: machine.name.clone(), resident: resident.clone() };
    match link.forget(&account, &conversation).await {
        Ok(true) => Ok(How::Asked),
        Ok(false) => {
            let ask = Ask { hub: link.membership().id.clone(), account, conversation, after: None };
            keep(node, vec![ask])
                .await
                .map_err(|error| Failure::new(format!("keeping what to ask the hub: {error}")))?;
            node.forget_asks.wake.notify_one();
            Ok(How::Queued)
        }
        Err(reason) => Err(Failure::new(reason)),
    }
}

pub(crate) fn wake(node: &Node) {
    node.forget_asks.wake.notify_one();
}

/// Recheck deleted originals after this conversation releases its inherited references.
pub(crate) async fn release_attachments(node: &Arc<Node>, id: &ResidentId) -> Result<Vec<Address>, String> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(Vec::new()) };
    let owners = crate::attachments::lifetime::releasing(node, id).map_err(|error| error.message)?;
    let mut asks = Vec::new();
    for conversation in &owners {
        for (hub, account) in pairs(node, &about.acting) {
            asks.push(Ask { hub, account, conversation: conversation.clone(), after: Some(id.clone()) });
        }
    }
    if !asks.is_empty() {
        keep(node, asks).await.map_err(|error| error.to_string())?;
    }
    Ok(owners)
}

/// Deletes an owner's conversation as part of the hub's project deletion.
/// The cross-machine cleanup is durable before any local data is removed.
pub(crate) async fn project(
    node: &Arc<Node>,
    id: &ResidentId,
    hub: &HubId,
    account: &AccountName,
) -> Result<(), String> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    if !node.serves(&about.acting, hub, account) {
        return Err(format!("{id} does not work for the account {account}"));
    }
    let mut accounts = pairs(node, &about.acting);
    if !accounts.contains(&(hub.clone(), account.clone())) {
        accounts.push((hub.clone(), account.clone()));
    }
    keep(
        node,
        accounts
            .into_iter()
            .map(|(hub, account)| Ask {
                hub,
                account,
                conversation: Address { machine: node.machine(), resident: id.clone() },
                after: Some(id.clone()),
            })
            .collect(),
    )
    .await
    .map_err(|error| error.to_string())?;
    home(node, id).await?;
    node.forget_asks.wake.notify_one();
    Ok(())
}

/// The hub `hub` has this machine delete what it keeps of a conversation.
pub(crate) async fn told(node: &Arc<Node>, hub: &HubId, forgetting: Forgetting) -> Forgot {
    let Ok(_call) = handing(node) else {
        return Forgot::Failed { reason: format!("the node on {} is handing over to its successor", node.machine()) };
    };
    let Forgetting { conversation, account, .. } = forgetting;
    if !node.link_of(hub).is_some_and(|link| link.view().is_me(&conversation.machine)) {
        return match elsewhere(node, hub, &account, &conversation).await {
            Ok(()) => Forgot::Done,
            Err(reason) => Forgot::Failed { reason },
        };
    }
    let id = &conversation.resident;
    // Of one no longer on record, nothing says whom it worked for: what is
    // left of it here goes all the same.
    let acting = node.residents.about(id).map(|(about, _)| about.acting);
    if let Some(acting) = &acting
        && !node.serves(acting, hub, &account)
    {
        return Forgot::Refused { reason: format!("{id} does not work for the account {account}") };
    }
    if let Err(reason) = home(node, id).await {
        return Forgot::Failed { reason };
    }
    node.log.line(format_args!("deleted {id}, which lived here, as the account {account} asked"));
    // The other accounts it worked for may have devices keeping a copy of
    // it: their hubs are asked too.
    let others: Vec<Ask> = acting
        .map(|acting| pairs(node, &acting))
        .unwrap_or_default()
        .into_iter()
        .filter(|(other, other_account)| !(other == hub && *other_account == account))
        .map(|(hub, account)| Ask {
            hub,
            account,
            conversation: Address { machine: node.machine(), resident: id.clone() },
            after: None,
        })
        .collect();
    if !others.is_empty() {
        match keep(node, others).await {
            Ok(()) => node.forget_asks.wake.notify_one(),
            Err(error) => node.log.line(format_args!("keeping what to ask the other hubs of {id}: {error}")),
        }
    }
    Forgot::Done
}

/// Asks the hubs what is still to be asked, whenever a hub's view changes,
/// as when a link connects, or a new ask is kept.
pub(crate) fn start(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let mut views = node.views.subscribe();
    let wake = node.forget_asks.wake.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let woken = wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let done = {
                let Some(node) = weak.upgrade() else { return };
                send_kept(&node).await
            };
            if done {
                again.reset();
            }
            tokio::select! {
                () = tokio::time::sleep(again.next()), if !done => {},
                changed = views.changed() => if changed.is_err() { return },
                () = woken => {},
            }
        }
    });
}

fn handing(node: &Arc<Node>) -> Result<crate::handoff::Busy, Failure> {
    node.handoff.call().ok_or_else(|| {
        Failure::new(format!("the node on {} is handing over to its successor; ask again in a moment", node.machine()))
    })
}

/// Every hub and account `acting` works for, of the hubs this computer is
/// on.
fn pairs(node: &Node, acting: &Acting) -> Vec<(HubId, AccountName)> {
    node.links()
        .iter()
        .flat_map(|link| {
            let hub = link.membership().id.clone();
            node.accounts_on(link, acting).into_iter().map(move |account| (hub.clone(), account))
        })
        .collect()
}

/// Deletes the conversation `id`, which lives here, and everything of it
/// here: from now on it takes nothing new, and its turn and its commands
/// are stopped first.
async fn home(node: &Arc<Node>, id: &ResidentId) -> Result<(), String> {
    let _deleting = node.residents.deleting(id);
    stop_turn(node, id).await?;
    let address = Address { machine: node.machine(), resident: id.clone() };
    stop_jobs(node, &address).await?;
    match crate::data::delete::delete_one(node, id.as_str()).await {
        Ok(_) | Err(Stop::Nothing(_)) => {}
        Err(stop) => return Err(stop.why()),
    }
    let names = node.machine_names();
    let mut machines = vec![names.name];
    machines.extend(names.was);
    forget_copies(node, id, machines).await
}

/// Stops the turn `id` runs, for its deletion. A Claude Code background
/// session the World started whose turn does not end as asked, or whose
/// plugin cannot be asked, is stopped as a whole, and its turn with it.
async fn stop_turn(node: &Arc<Node>, id: &ResidentId) -> Result<(), String> {
    let asked = match tokio::time::timeout(TURN_STOPS, crate::talk::stop_and_wait(node, id)).await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(failure)) => format!("stopping its turn: {}", failure.message),
        Err(_) => format!("its turn did not stop within {} seconds of being asked", TURN_STOPS.as_secs()),
    };
    let Some((about, _)) = node.residents.about(id) else { return Ok(()) };
    let Some(_) = about.session.as_deref().filter(|_| about.world_started && about.host.agent.as_str() == "claude")
    else {
        return Err(asked);
    };
    crate::lifecycle::release(node, id, crate::lifecycle::ReleaseReason::StopForDelete)
        .await
        .map_err(|failure| format!("{asked}; stopping its Claude Code session: {}", failure.message))?;
    if node.residents.now(id).is_some_and(|now| now.turn.is_some()) {
        return Err(format!("{asked}; its turn did not end with its Claude Code session"));
    }
    Ok(())
}

/// How long a turn is given to stop when its conversation is deleted.
const TURN_STOPS: std::time::Duration = std::time::Duration::from_secs(20);

/// Deletes the guest rooms here of the conversation at `conversation`, its
/// commands in them stopped first.
async fn elsewhere(node: &Arc<Node>, hub: &HubId, account: &AccountName, conversation: &Address) -> Result<(), String> {
    let names = node.names();
    let mut machines = names.names_of(&conversation.machine);
    if machines.is_empty() {
        machines.push(conversation.machine.clone());
    }
    let world = node.world.clone();
    let wanted = (conversation.resident.clone(), machines.clone());
    let rooms: Vec<(String, Address)> = tokio::task::spawn_blocking(move || -> std::io::Result<_> {
        let (resident, machines) = wanted;
        Ok(world
            .kept()?
            .into_iter()
            .filter_map(|kept| {
                let machine = machines.iter().find(|machine| kept.kind.is_room_of(&resident, machine))?;
                Some((kept.name, Address { machine: machine.clone(), resident: resident.clone() }))
            })
            .collect())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("finding its rooms here: {error}"))?;
    for (name, owner) in rooms {
        stop_jobs(node, &owner).await?;
        crate::attachments::lifetime::remember(node.world.owner(&node.names(), &owner).path(), hub, account)
            .map_err(|error| error.message)?;
        match crate::data::delete::delete_one(node, &name).await {
            Ok(_) | Err(Stop::Nothing(_)) => {}
            Err(stop) => return Err(stop.why()),
        }
        node.log.line(format_args!("deleted {name}, the room here of a conversation deleted"));
    }
    forget_copies(node, &conversation.resident, machines).await
}

/// Stops every command of `owner` still running here.
async fn stop_jobs(node: &Arc<Node>, owner: &Address) -> Result<(), String> {
    let (world, names, owner) = (node.world.clone(), node.names(), owner.clone());
    let running: Vec<PathBuf> = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<PathBuf>> {
        let mut running = Vec::new();
        for directory in world.job_directories(&names, &owner)? {
            let records = match std::fs::read_dir(&directory) {
                Ok(entries) => {
                    entries.map(|entry| entry.map(|entry| entry.path())).collect::<std::io::Result<Vec<_>>>()?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for record in records {
                if record.extension().is_some_and(|extension| extension == "json")
                    && let Ok(job) = fastctx_hands::run::record::read(&record)
                    && job.end.is_none()
                {
                    running.push(record);
                }
            }
        }
        Ok(running)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("finding its commands: {error}"))?;
    for record in running {
        crate::jobs::stop(node, &record, "the user", "its conversation was deleted")
            .await
            .map_err(|failure| format!("stopping a command it started: {}", failure.message))?;
    }
    Ok(())
}

/// Forgets the copy this computer kept of the conversation of `resident`
/// from showing it, under any name its machine had.
async fn forget_copies(node: &Arc<Node>, resident: &ResidentId, machines: Vec<MachineName>) -> Result<(), String> {
    let (root, resident) = (node.root.clone(), resident.clone());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let cache = fastctx_client::cache::Cache::under(&root)?;
        for machine in machines {
            cache.forget(&Address { machine, resident: resident.clone() })?;
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("forgetting the copy kept here from showing it: {error}"))
}

fn kept_file(node: &Node) -> PathBuf {
    node.world.path().join("forget-asks.json")
}

/// Keeps `asks` to be asked of their hubs, beside those kept already.
async fn keep(node: &Arc<Node>, asks: Vec<Ask>) -> std::io::Result<()> {
    let _file = node.forget_asks.file.lock().await;
    let path = kept_file(node);
    tokio::task::spawn_blocking(move || {
        let mut kept = read(&path)?;
        for ask in asks {
            if !kept.contains(&ask) {
                kept.push(ask);
            }
        }
        write(&path, &kept)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Asks each hub connected now what is still to be asked of it; says
/// whether nothing is left to ask.
async fn send_kept(node: &Arc<Node>) -> bool {
    let _file = node.forget_asks.file.lock().await;
    let path = kept_file(node);
    let reading = path.clone();
    let kept = match tokio::task::spawn_blocking(move || read(&reading)).await {
        Ok(Ok(kept)) => kept,
        Ok(Err(error)) => {
            node.log.line(format_args!("reading the deletions to ask the hubs: {error}"));
            return false;
        }
        Err(error) => {
            node.log.line(format_args!("reading the deletions to ask the hubs: {error}"));
            return false;
        }
    };
    drop(_file);
    if kept.is_empty() {
        return true;
    }
    let mut left = Vec::new();
    for ask in &kept {
        if ask.after.as_ref().is_some_and(|id| node.residents.deleted(id).is_none()) {
            left.push(ask.clone());
            continue;
        }
        let bytes = serde_json::to_vec(ask).expect("deletion request serializes");
        let id = crate::speech::fixed_id("forget", &[&fastctx_hands::files::content_hash(&bytes)]);
        let terminal = node.world.path().join("forget-outcomes").join(format!("{id}.json"));
        if let Err(error) = fastctx_platform::ensure_private_dir(terminal.parent().expect("deletion outcomes")) {
            node.log.line(format_args!("keeping deletion outcomes: {error}"));
            left.push(ask.clone());
            continue;
        }
        match crate::obligations::finished(&terminal, &bytes) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => {
                let _ = crate::obligations::retry(node, &terminal, &error.to_string());
                left.push(ask.clone());
                continue;
            }
        }
        let outcome = match node.link_of(&ask.hub) {
            None => match crate::obligations::hub_left(node, &ask.hub) {
                Ok(true) => {
                    Ok(Some((crate::obligations::Outcome::Impossible, "the network was left or removed".to_string())))
                }
                Ok(false) => Err("the hub is not connected; the deletion remains pending".to_string()),
                Err(error) => Err(error.to_string()),
            },
            Some(link) => match link.forget(&ask.account, &ask.conversation).await {
                Ok(true) => {
                    Ok(Some((crate::obligations::Outcome::Delivered, "the hub accepted the deletion".to_string())))
                }
                Ok(false) => Ok(None),
                Err(reason) => Err(reason),
            },
        };
        match outcome {
            Ok(Some((outcome, reason))) => {
                if let Err(error) = crate::obligations::finish(node, &terminal, &bytes, outcome, &reason) {
                    node.log.line(format_args!("settling a deletion request: {error}"));
                    left.push(ask.clone());
                }
            }
            Ok(None) => left.push(ask.clone()),
            Err(reason) => {
                if let Err(error) = crate::obligations::retry(node, &terminal, &reason) {
                    node.log.line(format_args!("recording a deletion retry: {error}"));
                }
                left.push(ask.clone());
            }
        }
    }
    // New deletions may have arrived while the hubs were being asked.
    let _file = node.forget_asks.file.lock().await;
    match tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut current = read(&path)?;
        current.retain(|ask| !kept.contains(ask) || left.contains(ask));
        write(&path, &current)?;
        Ok(current.is_empty())
    })
    .await
    {
        Ok(Ok(done)) => done,
        result => {
            let error = match result {
                Ok(Err(error)) => error.to_string(),
                Err(error) => error.to_string(),
                Ok(Ok(_)) => unreachable!(),
            };
            node.log.line(format_args!("keeping the deletions still to ask: {error}"));
            false
        }
    }
}

fn read(path: &std::path::Path) -> std::io::Result<Vec<Ask>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write(path: &std::path::Path, asks: &[Ask]) -> std::io::Result<()> {
    if asks.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(asks).map_err(std::io::Error::other)?;
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)
}
