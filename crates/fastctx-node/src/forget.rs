//! Deleting a conversation from every machine and device keeping something
//! of it (`world-colleagues.md`, 用户在任何地方和居民说话).
//!
//! The user asks from any computer or phone. The request is recorded before
//! a local conversation is stopped and erased, and each hub it works for is then
//! asked to have its other members delete what they keep. One that lives
//! elsewhere is asked of its hub, which has its machine delete it first. A
//! hub this computer is not connected to now is asked once it is: what is
//! still to be asked is kept in `forget-asks.json` beside the World.
//!
//! Told by a hub (`Down::Forget`), this machine deletes what it keeps: of a
//! conversation that lives here, all of it, once it is sure the conversation
//! works for the account the deletion is for; of one that lives elsewhere,
//! the guest rooms it had here, its commands there stopped first. Either
//! way, the copy kept here from showing it goes too. What the asset library
//! holds of it goes when the hub, having deleted its scopes, says they are
//! read no more (`assets::session`); what an earlier deletion left there is
//! asked of the hub again (`leftover`).

mod colleagues;
mod leftover;
mod submission;

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

/// What is still to be asked of hubs.
#[derive(Default)]
pub(crate) struct Asks {
    /// Held while the kept asks are read and written.
    file: crate::ranked::Forgetting,
}

/// Sending the kept asks to their hubs is a pass (`crate::passes`), made
/// when an ask is kept and when a hub's view changes.
const ASKING: crate::passes::Pass =
    crate::passes::Pass { name: "forget/asks", within: std::time::Duration::from_secs(10 * 60), rests: None };
/// How long a hub's view is quiet before the asks are sent again.
const VIEWS_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// How long a deletion is asked of a hub before it is given up: by then the
/// rooms other machines kept for the conversation have gone of themselves.
const ASKED_FOR_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// A deletion still to be asked of a hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Ask {
    hub: HubId,
    account: AccountName,
    conversation: Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<ResidentId>,
    /// The day an asking for what a deletion left behind is of (`leftover`):
    /// asked again another day, it is another asking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    round: Option<i64>,
    /// When it was kept, in milliseconds; absent in asks kept by earlier
    /// releases until first sent. Not part of what is asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since: Option<i64>,
}

impl PartialEq for Ask {
    fn eq(&self, other: &Self) -> bool {
        (&self.hub, &self.account, &self.conversation, &self.after, &self.round)
            == (&other.hub, &other.account, &other.conversation, &other.after, &other.round)
    }
}

impl Eq for Ask {}

impl Ask {
    /// What is asked, as its outcome is known by: without when it was kept.
    fn asked(&self) -> Vec<u8> {
        serde_json::to_vec(&Ask { since: None, ..self.clone() }).expect("deletion request serializes")
    }

    fn ready(&self, residents: &crate::residents::Residents) -> bool {
        self.after.as_ref().is_none_or(|id| residents.ever_deleted(id))
    }
}

/// What whoever asks about `resident`, which is not here, is told: that it
/// was deleted, which asking again will not change, only when its tombstone
/// says so. Not found without one, it is only not here: the asker may have
/// the wrong machine, or this one may have lost track of it, and neither is
/// a deletion (`world-gui.md`, 对话什么时候算删了).
pub(crate) fn gone(node: &Node, resident: &ResidentId) -> Failure {
    match node.residents.deleted(resident) {
        Some(at) => Failure::deleted(format!("{resident} was deleted from {} at {at}", node.machine())),
        None => Failure::not_run(format!("no conversation {resident} is on {}", node.machine())),
    }
}

/// The user asks, on this computer, that the conversation of `resident` on
/// `machine` be deleted from every machine and device keeping something of
/// it.
pub(crate) async fn ask(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: fastctx_types::access::Written,
) -> Result<How, Failure> {
    submission::ask(node, resident, machine).await
}

fn allowed_here(node: &Node, resident: &ResidentId) -> Result<(), Failure> {
    if node.residents.about(resident).is_some()
        && !crate::admission::seen::resident(node, crate::admission::seen::Looker::Here(&Acting::Here), resident)
    {
        return Err(Failure::refused("this account does not own that conversation"));
    }
    Ok(())
}

async fn ask_here(node: &Arc<Node>, resident: ResidentId) -> Result<How, Failure> {
    let _call = handing(node)?;
    allowed_here(node, &resident)?;
    let asking = delete_here(node, &resident).await?;
    node.log.line(format_args!("deleted {resident}, which lived here, as the user asked"));
    if !asking {
        // The asking before this one may have kept what to ask the hubs and
        // not come as far as asking it.
        wake(node);
        return Ok(How::Deleted);
    }
    Ok(if send_kept(node).await { How::Asked } else { How::Queued })
}

/// Deletes `resident`, which lives here, from every machine and device
/// keeping something of it: what to ask each hub it works for is kept, held
/// back until it is gone here, and then it is deleted here. Says whether
/// anything was kept to be asked.
async fn delete_here(node: &Arc<Node>, resident: &ResidentId) -> Result<bool, Failure> {
    let address = Address { machine: node.machine(), resident: resident.clone() };
    for link in node.links().into_iter().filter(|link| link.view().shared_records) {
        if node.residents.about(resident).is_none() {
            continue;
        }
        if let Ok((actor, current)) = link.record_actor(&address).await {
            link.prepare_forget(&actor, &current).await.map_err(Failure::new)?;
        }
    }
    // Whom it works for is read before its records go.
    let asks: Vec<Ask> = match node.residents.about(resident) {
        Some((about, _)) => pairs(node, &about.acting)
            .into_iter()
            .map(|(hub, account)| Ask {
                hub,
                account,
                conversation: address.clone(),
                after: Some(resident.clone()),
                round: None,
                since: None,
            })
            .collect(),
        None => Vec::new(),
    };
    if !asks.is_empty() {
        // The sender's after barrier prevents a saved request from overtaking home.
        keep(node, asks.clone())
            .await
            .map_err(|error| Failure::not_run(format!("keeping what to ask the hubs: {error}")))?;
    }
    // Asked again of a conversation deleted already, this finishes what a
    // deletion that failed or was interrupted part way left here: deleting
    // what is gone deletes nothing, and the rest is done as the first time.
    home(node, resident).await.map_err(Failure::new)?;
    if !asks.is_empty()
        && let Err(error) = keep(node, asks.clone()).await
    {
        // Kept again: a deletion of it refused meanwhile may have taken back
        // what the two shared.
        node.log.line(format_args!("keeping what to ask the hubs of {resident}: {error}"));
    }
    Ok(!asks.is_empty())
}

async fn ask_elsewhere(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    account: AccountName,
    conversation: Address,
) -> Result<How, Failure> {
    let _call = handing(node)?;
    match link.forget(&account, &conversation).await {
        Ok(true) => Ok(How::Asked),
        Ok(false) => {
            let ask =
                Ask { hub: link.membership().id.clone(), account, conversation, after: None, round: None, since: None };
            keep(node, vec![ask])
                .await
                .map_err(|error| Failure::new(format!("keeping what to ask the hub: {error}")))?;
            wake(node);
            Ok(How::Queued)
        }
        Err(reason) => Err(Failure::new(reason)),
    }
}

/// Asks for the kept asks to be sent.
pub(crate) fn wake(node: &Node) {
    crate::passes::ask(node, ASKING.name);
}

/// Recheck deleted originals after this conversation releases its inherited references.
pub(crate) async fn release_attachments(node: &Arc<Node>, id: &ResidentId) -> Result<Vec<Address>, String> {
    let Some((about, _)) = node.residents.about(id) else { return Ok(Vec::new()) };
    let owners = crate::attachments::lifetime::releasing(node, id).await.map_err(|error| error.message)?;
    let mut asks = Vec::new();
    for conversation in &owners {
        for (hub, account) in pairs(node, &about.acting) {
            asks.push(Ask {
                hub,
                account,
                conversation: conversation.clone(),
                after: Some(id.clone()),
                round: None,
                since: None,
            });
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
                round: None,
                since: None,
            })
            .collect(),
    )
    .await
    .map_err(|error| error.to_string())?;
    home(node, id).await?;
    wake(node);
    Ok(())
}

/// How long a deletion the hub asks for is tried again here while it is
/// refused for a moment, as while a file arrives for the conversation or its
/// turn winds down. The hub asks again only at this machine's next session,
/// so a refusal answered at once would hold the deletion until then.
const TOLD_TRIES_FOR: std::time::Duration = std::time::Duration::from_secs(120);
const TOLD_PAUSE_FIRST: std::time::Duration = std::time::Duration::from_secs(1);
const TOLD_PAUSE_MOST: std::time::Duration = std::time::Duration::from_secs(15);

/// The hub `hub` has this machine delete what it keeps of a conversation.
pub(crate) async fn told(node: &Arc<Node>, hub: &HubId, forgetting: Forgetting) -> Forgot {
    let began = tokio::time::Instant::now();
    let mut pause = TOLD_PAUSE_FIRST;
    let mut said: Option<String> = None;
    loop {
        let reason = match told_once(node, hub, &forgetting).await {
            Ok(outcome) => return outcome,
            Err(reason) => reason,
        };
        let key = forgetting.conversation.key();
        if began.elapsed() + pause > TOLD_TRIES_FOR {
            node.log.line(format_args!(
                "deleting {key} as its hub asked did not go: {reason}; the hub asks again at the next session"
            ));
            return Forgot::Failed { reason };
        }
        if said.as_deref() != Some(reason.as_str()) {
            node.log.line(format_args!("deleting {key} as its hub asked: {reason}; trying again"));
            said = Some(reason);
        }
        tokio::time::sleep(pause).await;
        pause = (pause * 2).min(TOLD_PAUSE_MOST);
    }
}

/// One try of `told`: how it ended, or why it may go on a later try.
async fn told_once(node: &Arc<Node>, hub: &HubId, forgetting: &Forgetting) -> Result<Forgot, String> {
    let Ok(_call) = handing(node) else {
        return Ok(Forgot::Failed {
            reason: format!("the node on {} is handing over to its successor", node.machine()),
        });
    };
    let Forgetting { conversation, account, .. } = forgetting;
    let id = &conversation.resident;
    // One on record here lives here, whatever the view of the hub held now
    // says, as before a new session's view came.
    let here = node.residents.on_record(id).is_some()
        || node.link_of(hub).is_some_and(|link| link.view().is_me(&conversation.machine));
    if !here {
        return elsewhere(node, hub, account, conversation).await.map(|()| Forgot::Done);
    }
    // Of one no longer on record, nothing says whom it worked for: what is
    // left of it here goes all the same.
    let acting = node.residents.about(id).map(|(about, _)| about.acting);
    if let Some(acting) = &acting
        && !node.serves(acting, hub, account)
    {
        return Ok(Forgot::Refused { reason: format!("{id} does not work for the account {account}") });
    }
    home(node, id).await?;
    node.log.line(format_args!("deleted {id}, which lived here, as the account {account} asked"));
    // The other accounts it worked for may have devices keeping a copy of
    // it: their hubs are asked too.
    let others: Vec<Ask> = acting
        .map(|acting| pairs(node, &acting))
        .unwrap_or_default()
        .into_iter()
        .filter(|(other, other_account)| !(other == hub && other_account == account))
        .map(|(hub, account)| Ask {
            hub,
            account,
            conversation: Address { machine: node.machine(), resident: id.clone() },
            after: None,
            round: None,
            since: None,
        })
        .collect();
    if !others.is_empty() {
        match keep(node, others).await {
            Ok(_) => wake(node),
            Err(error) => node.log.line(format_args!("keeping what to ask the other hubs of {id}: {error}")),
        }
    }
    Ok(Forgot::Done)
}

/// Names what sends the kept asks, before the node's jobs start: now, and
/// whenever a hub's view changes, as when a link connects. A new ask kept
/// asks for a sending itself (`wake`).
pub(crate) fn start(node: &Arc<Node>) {
    use crate::passes::Left;
    submission::register(node);
    colleagues::begin();
    crate::passes::register(node, ASKING, |node: Arc<Node>| async move {
        let more = leftover::look(&node).await;
        // The asks first: no colleague here keeps them waiting. Those a
        // colleague deleted here adds go after it.
        let mut sent = send_kept(&node).await;
        let swept = colleagues::sweep(&node, None).await;
        if swept.deleted {
            sent = send_kept(&node).await;
        }
        if sent && !more && !swept.left { Left::Nothing } else { Left::Work }
    });
    crate::passes::follow(node, ASKING.name, node.views.subscribe(), VIEWS_SETTLE);
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
    // Which colleagues it asked is read while its records are here, and
    // what each is left to is kept before it goes.
    let plan = colleagues::plan(node, id, false).await?;
    match crate::data::delete::delete_one(node, id.as_str()).await {
        Ok(_) | Err(Stop::Nothing(_)) => {}
        Err(stop) => {
            plan.take_back(node, id).await;
            return Err(stop.why());
        }
    }
    node.context_bodies.remove(id);
    let names = node.machine_names();
    let mut machines = vec![names.name];
    machines.extend(names.was);
    // Gone here: its colleagues follow, whatever becomes of the copy kept
    // here from showing it.
    let copies = forget_copies(node, id, machines).await;
    plan.follow(node, id).await;
    copies
}

/// Whether `id` is gone or going here: deleted, or its deletion begun.
fn going(node: &Node, id: &ResidentId) -> bool {
    node.residents.ever_deleted(id)
        || fastctx_platform::disk::exists(node.world.conversation(id).path().join(crate::data::delete::INTENT))
            .unwrap_or(true)
}

/// Takes back `added`, kept to be asked once `id` is gone here, where `id`
/// is neither gone nor going after all; nothing kept before it.
async fn take_back(node: &Arc<Node>, id: &ResidentId, added: Vec<Ask>) {
    if added.is_empty() || going(node, id) {
        return;
    }
    if let Err(error) = withdraw(node, added).await {
        node.log.line(format_args!("taking back what to ask the hubs of {id}: {error}"));
    }
}

/// Deletes `id`, which lives here and was idle longer than this machine's
/// retention keeps it (`conversation_retention::expiry`): here first, refused
/// rather than stopped where anything of it is under way or `still` says it
/// is not to go, and only then are the hubs it worked for asked to have every
/// other machine let go of what it keeps of it. What to ask them is kept
/// before anything of it goes, held back until it is gone here
/// (`Ask::ready`): a node stopped part way asks them once its deletion is
/// finished; a deletion refused takes it back.
pub(crate) async fn expired(
    node: &Arc<Node>,
    id: &ResidentId,
    still: crate::data::delete::Still,
) -> Result<(), String> {
    let _call = handing(node).map_err(|failure| failure.message)?;
    let address = Address { machine: node.machine(), resident: id.clone() };
    let asks: Vec<Ask> = match node.residents.about(id) {
        Some((about, _)) => pairs(node, &about.acting)
            .into_iter()
            .map(|(hub, account)| Ask {
                hub,
                account,
                conversation: address.clone(),
                after: Some(id.clone()),
                round: None,
                since: None,
            })
            .collect(),
        None => Vec::new(),
    };
    let asking = !asks.is_empty();
    let asks_again = asks.clone();
    let added = if asking {
        keep(node, asks).await.map_err(|error| format!("keeping what to ask the hubs: {error}"))?
    } else {
        Vec::new()
    };
    // Its colleagues follow it once it is gone: which they are is read
    // while its records are here. One of them here at work keeps it, as
    // anything of its own on its way would.
    let plan = match colleagues::plan(node, id, true).await {
        Ok(plan) => plan,
        Err(why) => {
            take_back(node, id, added).await;
            return Err(why);
        }
    };
    if let Some(colleague) = plan.at_work(node).await {
        take_back(node, id, added).await;
        plan.take_back(node, id).await;
        return Err(format!("its colleague {colleague} is at work"));
    }
    match crate::data::delete::delete_when(node, id.as_str(), Some(still)).await {
        Ok(_) | Err(Stop::Nothing(_)) => {}
        Err(stop) => {
            // Not deleted, nor being deleted: what this kept is taken back,
            // and nothing kept before it.
            take_back(node, id, added).await;
            plan.take_back(node, id).await;
            return Err(stop.why());
        }
    }
    node.context_bodies.remove(id);
    let names = node.machine_names();
    let mut machines = vec![names.name];
    machines.extend(names.was);
    let copies = forget_copies(node, id, machines).await;
    if asking {
        // Kept again: a deletion of it refused meanwhile may have taken back
        // what the two shared.
        if let Err(error) = keep(node, asks_again).await {
            node.log.line(format_args!("keeping what to ask the hubs of {id}: {error}"));
        }
        wake(node);
    }
    plan.follow(node, id).await;
    copies
}

/// `home`, for a purge (`crate::purges`): nothing of it is asked of a hub as
/// the account it worked for, which may no longer let this machine act for
/// it (its hub lets go of what it keeps itself); what is still being let go
/// of here, where anything is, is said rather than taken for done.
pub(crate) async fn purge_home(node: &Arc<Node>, id: &ResidentId) -> Result<Option<String>, String> {
    let _deleting = node.residents.deleting(id);
    stop_turn(node, id).await?;
    let address = Address { machine: node.machine(), resident: id.clone() };
    stop_jobs(node, &address).await?;
    let marked = node.world.conversation(id).path().join(crate::data::delete::PURGING);
    fastctx_platform::publish(&marked, b"", fastctx_platform::Permissions::Private)
        .map_err(|error| format!("recording that it is deleted for a purge: {error}"))?;
    let finishing = match crate::data::delete::delete_one(node, id.as_str()).await {
        Ok(gone) => gone.finishing,
        Err(Stop::Nothing(_)) => None,
        Err(stop) => {
            // Not deleted: one deleted later for another reason is deleted
            // as any is.
            if let Err(error) = fastctx_platform::disk::remove_file(&marked) {
                node.log.line(format_args!("{id}: {error}"));
            }
            return Err(stop.why());
        }
    };
    node.context_bodies.remove(id);
    purge_copies(node, id).await?;
    Ok(finishing)
}

/// Lets go of the copies here of `id`, which lived here, by every name this
/// machine had (`purge_home`); asked again of what is gone, it does nothing.
pub(crate) async fn purge_copies(node: &Arc<Node>, id: &ResidentId) -> Result<(), String> {
    let names = node.machine_names();
    let mut machines = vec![names.name];
    machines.extend(names.was);
    forget_copies(node, id, machines).await
}

/// Stops the turn `id` runs, for its deletion. A Claude Code background
/// session the World started whose turn does not end as asked, or whose
/// plugin cannot be asked, is stopped as a whole, and its turn with it.
async fn stop_turn(node: &Arc<Node>, id: &ResidentId) -> Result<(), String> {
    let asked = match tokio::time::timeout(TURN_STOPS, settle(node, id)).await {
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

/// Stops the turn `id` runs and waits for it to end; and one it is starting
/// whose id is not known yet (busy, no turn), as an answer it was just given
/// starts one, is given a moment to become a turn, stopped then, or to end,
/// which its deletion would otherwise be refused for as working
/// (`data::delete`). Nothing new starts for it meanwhile
/// (`Residents::deleting`). One whose words wait in line for its login starts
/// nothing until there is room, and one busy longer than that moment may
/// start nothing at all: those are left for the deletion to refuse, as
/// before.
async fn settle(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let mut changes = node.residents.changes();
    let starting = tokio::time::Instant::now() + TURN_STARTS;
    loop {
        let Some(now) = node.residents.now(id) else { return Ok(()) };
        if now.turn.is_some() {
            crate::talk::stop_and_wait(node, id).await?;
        } else if now.activity == fastctx_types::residents::Activity::Busy
            && crate::turn_slots::ahead(node, id).is_none()
        {
            match tokio::time::timeout_at(starting, changes.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(Failure::new("the node is stopping")),
                Err(_) => return Ok(()),
            }
        } else {
            return Ok(());
        }
    }
}

/// How long a turn is given to stop when its conversation is deleted.
const TURN_STOPS: std::time::Duration = std::time::Duration::from_secs(20);
/// How long a turn starting as its conversation is deleted is given to
/// become one, within `TURN_STOPS`.
const TURN_STARTS: std::time::Duration = std::time::Duration::from_secs(10);

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
    let rooms: Vec<(String, Address)> = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<_> {
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
    // A plan copy an earlier release registered for it would make its room
    // here again each time the node starts.
    if let Err(failure) = crate::placement::copies::guest_deleted(node, conversation, &machines) {
        node.log.line(format_args!("letting go of the plan copies of {}: {}", conversation.resident, failure.message));
    }
    // What its commands did here goes too, whether or not a room of it is left.
    let (sealing, resident, names) = (node.clone(), conversation.resident.clone(), machines.clone());
    fastctx_platform::task::spawn_blocking(move || crate::activity_book::seal_elsewhere(&sealing, &resident, &names))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("sealing its activity here: {error}"))?;
    for (name, owner) in rooms {
        stop_jobs(node, &owner).await?;
        {
            let (room, hub, account) =
                (node.world.owner(&node.names(), &owner).path().to_path_buf(), hub.clone(), account.clone());
            fastctx_platform::task::blocking(move || crate::attachments::lifetime::remember(&room, &hub, &account))
                .await
                .map_err(|error| error.message)?;
        }
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
    let running: Vec<PathBuf> = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<Vec<PathBuf>> {
        let mut running = Vec::new();
        for directory in world.job_directories(&names, &owner)? {
            let records = match fastctx_platform::disk::read_dir(&directory) {
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
    fastctx_platform::task::spawn_blocking(move || -> std::io::Result<()> {
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

/// Keeps `asks` to be asked of their hubs, beside those kept already; says
/// which it added.
async fn keep(node: &Arc<Node>, asks: Vec<Ask>) -> std::io::Result<Vec<Ask>> {
    let _file = node.forget_asks.file.lock().await;
    let path = kept_file(node);
    let now = jiff::Timestamp::now().as_millisecond();
    fastctx_platform::task::spawn_blocking(move || {
        let mut kept = read(&path)?;
        let mut added = Vec::new();
        for ask in asks {
            if !kept.contains(&ask) {
                kept.push(Ask { since: Some(now), ..ask.clone() });
                added.push(ask);
            }
        }
        write(&path, &kept)?;
        Ok(added)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Takes `asks` back from what is kept to be asked, as kept by `keep`.
async fn withdraw(node: &Arc<Node>, asks: Vec<Ask>) -> std::io::Result<()> {
    let _file = node.forget_asks.file.lock().await;
    let path = kept_file(node);
    fastctx_platform::task::spawn_blocking(move || {
        let mut kept = read(&path)?;
        let before = kept.len();
        kept.retain(|ask| !asks.contains(ask));
        if kept.len() == before {
            return Ok(());
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
    let kept = match fastctx_platform::task::spawn_blocking(move || read(&reading)).await {
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
    let now = jiff::Timestamp::now().as_millisecond();
    let mut left = Vec::new();
    // Those held back until their conversation is gone here: their month
    // counts from when they may be asked, not from when they were kept.
    let mut held_back = Vec::new();
    for ask in &kept {
        if !ask.ready(&node.residents) {
            held_back.push(ask.clone());
            left.push(ask.clone());
            continue;
        }
        let ask = &Ask { since: Some(ask.since.unwrap_or(now)), ..ask.clone() };
        let bytes = ask.asked();
        let given_up = now.saturating_sub(ask.since.unwrap_or(now)) > ASKED_FOR_MS;
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
            _ if given_up => {
                node.log.line(format_args!(
                    "giving up asking {} to delete {} for {}: asked for a month",
                    ask.hub, ask.conversation.resident, ask.account
                ));
                Ok(Some((crate::obligations::Outcome::Impossible, "asked for a month; given up".to_string())))
            }
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
                // The hub will not take it from this machine again: no such
                // account, this machine may not act for it, or no longer
                // signed in to it.
                Err(reason)
                    if crate::obligations::out_of_reach(&reason)
                        || reason.contains(" is not logged in to the account ") =>
                {
                    Ok(Some((crate::obligations::Outcome::Impossible, reason)))
                }
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
    match fastctx_platform::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut current = read(&path)?;
        current.retain(|ask| !kept.contains(ask) || left.contains(ask));
        // Those kept by an earlier release keep the time they were first sent.
        for ask in &mut current {
            if held_back.contains(ask) {
                ask.since = Some(now);
            } else if let Some(stamped) = left.iter().find(|stamped| *stamped == ask) {
                ask.since = ask.since.or(stamped.since);
            }
        }
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
    match fastctx_platform::disk::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write(path: &std::path::Path, asks: &[Ask]) -> std::io::Result<()> {
    if asks.is_empty() {
        return match fastctx_platform::disk::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec_pretty(asks).map_err(std::io::Error::other)?;
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)
}
