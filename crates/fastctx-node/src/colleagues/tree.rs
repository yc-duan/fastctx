//! Read a conversation's collaboration without starting model turns.

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, Purpose, Written},
    collaboration::{Branch, Tree},
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    residents::{Action, Origin},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::time::Instant;

pub(crate) async fn query(node: &Arc<Node>, id: ResidentId, machine: Option<Written>) -> Result<Tree, Failure> {
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

async fn remote(
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

fn visible(node: &Node, acting: &Acting, id: &ResidentId) -> Result<(), Failure> {
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
                            if let Some(account) = crate::collab::asked_account(&node, &parent, &address)? {
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
        if label.is_none() {
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
    let path = node.world.conversation(&owner.resident).actions();
    let actions = tokio::task::spawn_blocking(move || {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else { return Ok(Vec::new()) };
        bytes[..end]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Action>(line).map_err(std::io::Error::other))
            .collect::<std::io::Result<Vec<_>>>()
    })
    .await
    .map_err(|error| Failure::new(error.to_string()))?
    .map_err(|error| Failure::new(error.to_string()))?;
    let mut found = BTreeMap::new();
    for action in actions.into_iter().filter(|action| action.hand == "ask_colleague") {
        let address = Address {
            machine: action.machine,
            resident: action
                .result
                .parse()
                .map_err(|error| Failure::new(fastctx_types::collab_prompt::recorded_colleague(error)))?,
        };
        if let Some(account) = crate::collab::asked_account(node, &owner.resident, &address)?
            && !node
                .link_of(&account.hub)
                .is_some_and(|link| node.accounts_on(&link, acting).contains(&account.account))
        {
            continue;
        }
        found.insert(address, action.what);
    }
    for resident in node.residents.everyone() {
        if let Origin::Colleague { asked_by, label } = &resident.about.origin
            && asked_by.resident == owner.resident
            && node.is_me(&asked_by.machine)
            && visible(node, acting, &resident.id).is_ok()
        {
            found.insert(Address { machine: node.machine(), resident: resident.id }, label.clone());
        }
    }
    Ok(found)
}
