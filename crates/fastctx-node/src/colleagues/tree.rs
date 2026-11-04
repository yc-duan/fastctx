//! Read a conversation's collaboration without starting model turns.

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, HubAccount, Purpose, Written},
    collaboration::{Branch, Tree},
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    residents::Origin,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;

/// How long one read of a conversation's collaboration answers every ask
/// for it, at least: a page asks whenever the World changes, many times a
/// second while work goes on, and each read goes through every colleague's
/// record. A read that took long answers four times as long as it took, so
/// however often it is asked, reading it takes at most a fifth of the time.
const FRESH: Duration = Duration::from_secs(1);

/// How long a read no one asked again for is kept.
const KEPT: Duration = Duration::from_secs(60);

/// When a read was done, how long it took, and what it found.
type Read = Arc<tokio::sync::Mutex<Option<(Instant, Duration, Result<Tree, Failure>)>>>;

/// The latest read of each conversation's collaboration asked for, here or
/// from another machine, by conversation and by who asks: asks that come
/// together share one read.
#[derive(Default)]
pub(crate) struct Reads(std::sync::Mutex<std::collections::HashMap<String, Read>>);

/// The collaboration of `id` as the user here sees it, shared as `shared`
/// shares it.
pub(crate) async fn query(node: &Arc<Node>, id: ResidentId, machine: Option<Written>) -> Result<Tree, Failure> {
    let key = serde_json::to_string(&(&id, &machine)).map_err(|error| Failure::new(error.to_string()))?;
    shared(node, key, read_now(node, id, machine)).await
}

/// The collaboration of `id` here as `acting` on another machine sees it,
/// shared as `shared` shares it: an interface of any version asks for it
/// whole, as often as it likes.
pub(crate) async fn here_shared(
    node: &Arc<Node>,
    id: ResidentId,
    acting: Acting,
    colleagues: bool,
) -> Result<Tree, Failure> {
    let key =
        serde_json::to_string(&("asked", &id, &acting, colleagues)).map_err(|error| Failure::new(error.to_string()))?;
    shared(node, key, async move { here(node, &id, &acting, colleagues).await }).await
}

/// What `read` finds, read once for the asks of `key` that come while it is
/// fresh.
async fn shared(
    node: &Arc<Node>,
    key: String,
    read: impl std::future::Future<Output = Result<Tree, Failure>>,
) -> Result<Tree, Failure> {
    let kept = {
        let mut reads = node.collaboration_reads.0.lock().expect("never poisoned");
        reads.retain(|_, kept| {
            Arc::strong_count(kept) > 1
                || kept.try_lock().is_ok_and(|held| held.as_ref().is_some_and(|(at, _, _)| at.elapsed() < KEPT))
        });
        reads.entry(key).or_default().clone()
    };
    let mut held = kept.lock().await;
    if let Some((at, took, result)) = held.as_ref()
        && at.elapsed() < FRESH.max(*took * 4)
    {
        return result.clone();
    }
    let began = Instant::now();
    let result = read.await;
    *held = Some((Instant::now(), began.elapsed(), result.clone()));
    result
}

async fn read_now(node: &Arc<Node>, id: ResidentId, machine: Option<Written>) -> Result<Tree, Failure> {
    let acting = Acting::Here;
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.knows(&id) => node.machine().into(),
        None => super::find_for(node, &acting, &id).await?,
    };
    if node.is_here(&machine) {
        return here(node, &id, &acting, true).await;
    }
    remote(node, &acting, &machine, id, true).await
}

pub(super) async fn remote(
    node: &Arc<Node>,
    acting: &Acting,
    machine: &Written,
    id: ResidentId,
    colleagues: bool,
) -> Result<Tree, Failure> {
    let links = node.locate(machine)?;
    if !links.iter().any(|link| link.view().collaboration_tree) {
        return Err(Failure::not_run(fastctx_types::collab_prompt::TREE_HUB_OLD));
    }
    let (link, account) = links
        .into_iter()
        .find_map(|link| {
            let view = link.view();
            if !view.collaboration_tree {
                return None;
            }
            let account =
                view.account_for(&node.accounts_on(&link, acting), &view.me, &machine.name, Purpose::Agents)?;
            drop(view);
            Some((link, account))
        })
        .ok_or_else(|| Failure::refused(fastctx_types::collab_prompt::TREE_ACCOUNT))?;
    let reply = link
        .call(&machine.name, &account, Purpose::Look, RemoteCall::Facts)
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    let RemoteReply::Facts(facts) = reply else {
        return Err(Failure::new(fastctx_types::collab_prompt::TREE_CAPABILITIES));
    };
    if !facts.collaboration_tree {
        return Err(Failure::not_run(fastctx_types::collab_prompt::TREE_MACHINE_OLD));
    }
    let reply = link
        .call(&machine.name, &account, Purpose::Agents, RemoteCall::Collaboration { resident: id, colleagues })
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    let RemoteReply::Collaboration(tree) = reply else {
        return Err(Failure::new(fastctx_types::collab_prompt::TREE_UNANSWERED));
    };
    Ok(*tree)
}

pub(super) fn visible(node: &Node, acting: &Acting, id: &ResidentId) -> Result<(), Failure> {
    if crate::admission::seen::resident(node, crate::admission::seen::Looker::Here(acting), id) {
        Ok(())
    } else {
        Err(Failure::refused(fastctx_types::collab_prompt::TREE_INVISIBLE))
    }
}

pub(crate) async fn here(
    node: &Arc<Node>,
    id: &ResidentId,
    acting: &Acting,
    colleagues: bool,
) -> Result<Tree, Failure> {
    visible(node, acting, id)?;
    // The transport allows 150 seconds for a query that reads a host.
    let deadline = Instant::now() + Duration::from_secs(120);
    let owner = Address { machine: node.machine(), resident: id.clone() };
    let targets = if colleagues { targets(node, &owner, acting).await? } else { BTreeMap::new() };
    let conversation = branch(node, id, acting, deadline);
    let children = async {
        let mut pending = targets.into_iter();
        let mut reading = tokio::task::JoinSet::new();
        let mut branches = Vec::new();
        loop {
            while reading.len() < 4 {
                let Some((address, label)) = pending.next() else { break };
                let (node, acting, parent) = (node.clone(), acting.clone(), owner.resident.clone());
                reading.spawn(async move {
                    let query = async {
                        if node.is_me(&address.machine) {
                            branch(&node, &address.resident, &acting, deadline).await
                        } else {
                            let mut machine = Written::from(&address.machine);
                            if let Some(account) = crate::collab::asked_account(&node, &parent, &address).await? {
                                machine.hub = node.link_of(&account.hub).map(|link| link.hub_name());
                                let acting = Acting::one(account.hub, account.account);
                                remote(&node, &acting, &machine, address.resident.clone(), false)
                                    .await
                                    .map(|tree| tree.conversation)
                            } else {
                                remote(&node, &acting, &machine, address.resident.clone(), false)
                                    .await
                                    .map(|tree| tree.conversation)
                            }
                        }
                    };
                    match tokio::time::timeout_at(deadline, query).await {
                        Ok(Ok(branch)) => branch,
                        result => {
                            let why = match result {
                                Ok(Err(error)) => error.message,
                                _ => fastctx_types::collab_prompt::TREE_DEADLINE.into(),
                            };
                            Branch {
                                address,
                                label: Some(label),
                                born_at: None,
                                now: None,
                                goal: Err(why.clone()),
                                plan: None,
                                agents: Vec::new(),
                                unavailable: vec![why],
                            }
                        }
                    }
                });
            }
            match reading.join_next().await {
                Some(Ok(branch)) => branches.push(branch),
                Some(Err(error)) => return Err(Failure::new(error.to_string())),
                None => break,
            }
        }
        branches.sort_by(|a, b| a.address.cmp(&b.address));
        Ok(branches)
    };
    let (conversation, colleagues) = tokio::join!(conversation, children);
    visible(node, acting, id)?;
    Ok(Tree { conversation: conversation?, colleagues: colleagues? })
}

async fn branch(node: &Arc<Node>, id: &ResidentId, acting: &Acting, deadline: Instant) -> Result<Branch, Failure> {
    visible(node, acting, id)?;
    let (about, _) = node.residents.about(id).ok_or_else(|| crate::forget::gone(node, id))?;
    let label = match &about.origin {
        Origin::Colleague { label, .. } => Some(label.clone()),
        Origin::User => None,
    };
    let goal = async {
        if label.is_none() || !crate::goals::colleagues::offered() {
            return Ok(None);
        }
        match tokio::time::timeout_at(deadline, crate::goals::colleagues::summary(node, id)).await {
            Ok(result) => result.map_err(|error| error.message),
            Err(_) => Err(fastctx_types::collab_prompt::GOAL_DEADLINE.into()),
        }
    };
    let ((agents, unavailable), goal) = tokio::join!(super::native::read(node, id, deadline), goal);
    visible(node, acting, id)?;
    Ok(Branch {
        address: Address { machine: node.machine(), resident: id.clone() },
        label,
        born_at: Some(about.born_at),
        now: node.residents.now(id),
        goal,
        plan: Some(super::plan::access(&about)),
        agents,
        unavailable,
    })
}

async fn targets(node: &Arc<Node>, owner: &Address, acting: &Acting) -> Result<BTreeMap<Address, String>, Failure> {
    let mut found = asked(node, owner, acting).await?;
    found.extend(born_here(node, owner, acting));
    Ok(found)
}

/// The colleagues `owner` asked that `acting` may see, as its actions recorded
/// them, with what each was asked to do.
pub(super) async fn asked(
    node: &Arc<Node>,
    owner: &Address,
    acting: &Acting,
) -> Result<BTreeMap<Address, String>, Failure> {
    let mut found = BTreeMap::new();
    for (address, what, account) in recorded(node, owner).await? {
        if let Some(account) = account
            && !node
                .link_of(&account.hub)
                .is_some_and(|link| node.accounts_on(&link, acting).contains(&account.account))
        {
            continue;
        }
        found.insert(address, what);
    }
    Ok(found)
}

/// One colleague asked, what it was asked to do, and the account it was
/// asked for where one is recorded.
type Recorded = (Address, String, Option<HubAccount>);

/// What each conversation's actions said it asked, as last read, by the
/// version of its actions then.
type Remembered = std::sync::Mutex<HashMap<std::path::PathBuf, (u64, Vec<Recorded>)>>;

static RECORDED: std::sync::LazyLock<Remembered> = std::sync::LazyLock::new(Default::default);

/// The conversations whose asks are remembered at most: past it, all are
/// read again.
const RECORDED_MOST: usize = 4096;

/// The colleagues `owner` asked, as its actions record them. A page follows
/// a conversation's collaboration again at every turn's end, and reading
/// every ask and each one's account again cost a read of the database for
/// each colleague ever asked; now its actions are read again only once they
/// changed, and an account only while none is recorded for it.
async fn recorded(node: &Arc<Node>, owner: &Address) -> Result<Vec<Recorded>, Failure> {
    let path = node.world.conversation(&owner.resident).actions();
    let reading = path.clone();
    let version = fastctx_platform::task::spawn_blocking(move || crate::residents::activity::actions_version(&reading))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(error.to_string()))?;
    let kept =
        RECORDED.lock().expect("never poisoned").get(&path).filter(|kept| kept.0 == version).map(|kept| kept.1.clone());
    let mut recorded = match kept {
        Some(kept) => kept,
        None => {
            let reading = path.clone();
            let actions =
                fastctx_platform::task::spawn_blocking(move || crate::residents::activity::asked(&reading, None))
                    .await
                    .map_err(|error| Failure::new(error.to_string()))?
                    .map_err(|error| Failure::new(error.to_string()))?;
            let mut recorded = Vec::new();
            for action in actions.into_iter().filter(|action| action.hand == "ask_colleague") {
                let address = Address {
                    machine: action.machine,
                    resident: action
                        .result
                        .parse()
                        .map_err(|error| Failure::new(fastctx_types::collab_prompt::recorded_colleague(error)))?,
                };
                recorded.push((address, action.what, None));
            }
            recorded
        }
    };
    for (address, _, account) in &mut recorded {
        if account.is_none() {
            *account = crate::collab::asked_account(node, &owner.resident, address).await?;
        }
    }
    let mut remembered = RECORDED.lock().expect("never poisoned");
    if remembered.len() >= RECORDED_MOST && !remembered.contains_key(&path) {
        remembered.clear();
    }
    remembered.insert(path, (version, recorded.clone()));
    Ok(recorded)
}

/// The colleagues of `owner` that live on this machine and `acting` may see.
pub(super) fn born_here(node: &Node, owner: &Address, acting: &Acting) -> BTreeMap<Address, String> {
    let mut found = BTreeMap::new();
    for resident in node.residents.everyone() {
        if let Origin::Colleague { asked_by, label } = &resident.about.origin
            && asked_by.resident == owner.resident
            && node.is_me(&asked_by.machine)
            && visible(node, acting, &resident.id).is_ok()
        {
            found.insert(Address { machine: node.machine(), resident: resident.id }, label.clone());
        }
    }
    found
}
