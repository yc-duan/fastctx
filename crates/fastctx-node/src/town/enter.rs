//! Project entry is recorded at the conversation's home. A command in one
//! of its remote jobs returns this request through the authenticated hub.

use std::sync::Arc;

use fastctx_types::access::{Acting, HubAccount, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::project::MapView;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Address, Reply, ResidentId, TownName};

use super::caller::Caller;
use crate::Node;
use crate::residents::Heard;

pub(crate) async fn call(node: &Arc<Node>, caller: &Caller, town: TownName) -> Result<Reply, Failure> {
    let map = match caller {
        Caller::Resident(address) if node.is_me(&address.machine) => here(node, &address.resident, &town, None).await?,
        Caller::Resident(address) => remote(node, address, &town).await?,
        Caller::User => return Err(Failure::not_run("only an agent's resident conversation enters a project")),
        Caller::Page(_) => return Err(Failure::not_run("a page does not enter a project")),
        Caller::Session(_) => return Err(Failure::refused("the agent session has no resident identity yet")),
    };
    Ok(Reply::Entered { town, map: Box::new(map) })
}

async fn remote(node: &Arc<Node>, address: &Address, town: &TownName) -> Result<MapView, Failure> {
    let accounts = crate::jobs::running_for(&node.world, &node.names(), address);
    if accounts.is_empty() {
        return Err(Failure::refused("this command has no running job for the conversation"));
    }
    let mut choices = Vec::new();
    for account in accounts {
        let Some(link) = node.link_of(&account.hub) else { continue };
        link.refresh().await.map_err(Failure::new)?;
        let view = link.view();
        if view.project_named(&account.account, town).is_some()
            && view.admits(&view.me, &account.account, &address.machine, Purpose::Echo).is_ok()
        {
            choices.push((link, account));
        }
    }
    let (link, account) = match choices.len() {
        0 => {
            return Err(Failure::refused(format!(
                "the job's accounts may not enter the project {town} for this conversation"
            )));
        }
        1 => choices.pop().expect("one authorized route"),
        _ => return Err(Failure::refused("the project's hub is ambiguous for this job")),
    };
    if !link.view().project_entry {
        return Err(Failure::not_run(
            "the hub runs an older Fastctx that cannot enter projects from another machine; update it first",
        ));
    }
    let call = RemoteCall::EnterProject { resident: address.resident.clone(), town: town.clone() };
    match link
        .call(&address.machine, &account.account, Purpose::Echo, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::Entered { town: entered, map } if entered == *town => Ok(*map),
        _ => Err(Failure::new("the conversation's home answered something other than project entry")),
    }
}

pub(crate) async fn here(
    node: &Arc<Node>,
    id: &ResidentId,
    town: &TownName,
    requesting: Option<&HubAccount>,
) -> Result<MapView, Failure> {
    let gate = node.residents.management(id)?;
    let _writing = crate::gate_waits::alone(node, id, &gate).await?;
    node.residents.active(id)?;
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::refused("unknown conversation"))?;
    let acting = &about.acting;
    let requesting_actor = requesting.map(|account| Acting::one(account.hub.clone(), account.account.clone()));
    if let Some(account) = requesting {
        if !node.serves(acting, &account.hub, &account.account) {
            return Err(Failure::refused("the conversation does not work for the requesting account"));
        }
        super::access::check(node, requesting_actor.as_ref().expect("requesting actor"), town)?;
    }
    super::access::ready(node, acting, town).await?;
    if let Some(actor) = &requesting_actor {
        super::access::check(node, actor, town)?;
    }
    let moved = node
        .residents
        .enter(&node.world, id, town)
        .map_err(|error| Failure::new(format!("recording the project membership: {error}")))?;
    if moved {
        crate::codex::moved(node, id).await;
        super::instructions::hand_to_current(node, id).map_err(|error| {
            Failure::new(format!("membership changed, but refreshing project instructions failed: {error}"))
        })?;
    }
    let map = super::map::view_for(
        node,
        town,
        requesting_actor.as_ref().unwrap_or(acting),
        matches!(about.origin, fastctx_types::residents::Origin::Colleague { .. }),
    )
    .await?;
    let ledger = crate::ledger::seq(&node.world, town).map_err(|error| Failure::new(error.to_string()))?;
    node.residents
        .set_heard(id, Heard { scratch: None, context: None, user_prompt: None, town: Some(town.clone()), ledger });
    Ok(map)
}
