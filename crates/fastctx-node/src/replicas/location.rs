//! Select one existing replica to tell the model; selection never retires other copies.

pub(crate) mod selection;

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, HubAccount, Purpose, Written},
    ipc::Failure,
    placement::{Location, Resource},
    remote::{RemoteCall, RemoteReply},
    residents::Origin,
};
use std::sync::Arc;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

pub(crate) async fn prepare(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    acting: &Acting,
    account: &HubAccount,
    resource: Resource,
) -> Result<Option<Location>, Failure> {
    let link = node.link_of(&account.hub).ok_or_else(|| failure("the records' hub is unavailable"))?;
    let view = crate::authorization::current_view(&link)?;
    // A colleague works in the history of the conversation that asked it;
    // its plan, like every conversation's, is its own.
    let owner = match (origin, &resource) {
        (Origin::Colleague { asked_by, .. }, Resource::History { .. }) => asked_by.clone(),
        _ => Address { resident: id.clone(), machine: node.machine() },
    };
    let ownership = match &resource {
        Resource::History { scope: fastctx_types::history::Scope::Project { .. } } => None,
        _ => match link
            .owned_records(
                &account.account,
                fastctx_types::owned_records::Request::Resolve { conversation: owner.clone() },
            )
            .await
            .map_err(failure)?
        {
            fastctx_types::owned_records::Response::Conversation { conversation } => Some(conversation),
            _ => return Err(failure("the hub did not return the conversation's ownership")),
        },
    };
    let room = node.world.conversation(id);
    let focus = match node.residents.about(id) {
        Some((about, _)) => about.focus,
        None => crate::focus::opening(&room).await?.map(|opening| opening.focus).unwrap_or_default(),
    };
    let working_from = room.path().join("working-from.json");
    let speaking: Option<fastctx_types::MachineName> =
        match fastctx_platform::task::blocking(move || fastctx_platform::disk::read(working_from)).await {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(failure)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(failure(error)),
        };
    let ranked = candidates(node, &view, account, &resource, ownership.as_ref(), &focus, speaking.as_ref());
    let remember = |selected: Option<fastctx_types::MachineName>| {
        node.contexts.routes.observe(
            id,
            crate::context::routes::Route {
                account: account.clone(),
                resource: resource.clone(),
                ownership: ownership.clone(),
                speaking: speaking.clone(),
                selected,
                legacy: false,
            },
        )
    };
    remember(None);
    let subject = Acting::one(account.hub.clone(), account.account.clone());
    let mut outdated = false;
    for machine in ranked {
        if node.is_here(&machine) {
            let asker = crate::admission::Asker {
                machine: node.machine(),
                hub: account.hub.clone(),
                account: account.account.clone(),
                hands: true,
            };
            match crate::placement::prepare_copy(node, &asker, owner.clone(), 0, resource.clone()).await {
                Ok(location) => {
                    remember(Some(machine.name.clone()));
                    return Ok(Some(location));
                }
                Err(error) if error.message == crate::history::switch::COPIES_OFF => continue,
                Err(error) => return Err(error),
            }
        }
        match node.call(&subject, &machine, Purpose::Look, RemoteCall::Facts).await {
            Ok(Ok(RemoteReply::Facts(facts))) if facts.replicated_records => {}
            Ok(Ok(RemoteReply::Facts(_))) => {
                outdated = true;
                continue;
            }
            _ => continue,
        }
        match crate::record_changes::transport::on(
            node, &subject, &machine, Purpose::Hands,
            RemoteCall::WorkingCopy { owner: owner.clone(), placement: 0, resource: resource.clone() },
        )
        .await
        {
            Ok(RemoteReply::WorkingCopy { location }) => {
                remember(Some(machine.name.clone()));
                return Ok(Some(location));
            }
            Err(error) if error.message == crate::history::switch::COPIES_OFF => continue,
            Err(error) => return Err(error),
            _ => return Err(failure("the computer did not return the replica location")),
        }
    }
    let _ = acting;
    if outdated {
        Err(Failure::not_run("update an eligible computer to use replicated plans and history"))
    } else {
        Ok(None)
    }
}

pub(crate) async fn prepare_here(
    node: &Arc<Node>,
    asker: &crate::admission::Asker,
    owner: &Address,
    resource: &Resource,
) -> Result<Location, Failure> {
    let link = node.link_of(&asker.hub).ok_or_else(|| failure("the records' hub is unavailable"))?;
    let view = crate::authorization::current_view(&link)?;
    view.authorize(&asker.account, fastctx_net::authorization::Action::Work(&view.me)).map_err(Failure::from)?;
    if matches!(resource, Resource::History { .. })
        && !view
            .me()
            .and_then(|member| member.home.as_ref())
            .is_some_and(|account| crate::history::switch::on(&link, account))
    {
        return Err(Failure::not_run(crate::history::switch::COPIES_OFF));
    }
    let path = match resource {
        Resource::History { scope: fastctx_types::history::Scope::Project { project } } => {
            let project = view
                .project_named(&asker.account, project)
                .ok_or_else(|| failure("this account cannot use the project's history"))?;
            if !view.member_keeps_project(&view.me, &project.town) {
                return Err(Failure::refused("this computer does not keep that project's records"));
            }
            let scope =
                fastctx_types::replicas::Scope::Project { town: project.town.clone(), name: project.name.clone() };
            let path = super::sync::folder(node, &link, &scope).map_err(failure)?;
            let known = node.replicas.state.folders.lock().expect("never poisoned").contains_key(&path);
            if !known || !fastctx_platform::disk::is_dir(&path) {
                let account = view
                    .me()
                    .and_then(|member| member.home.clone())
                    .ok_or_else(|| failure("this computer has no registered account"))?;
                super::publication::folder(
                    node,
                    super::state::Folder {
                        account: HubAccount { hub: asker.hub.clone(), account },
                        scope,
                        path: path.clone(),
                    },
                )
                .await
                .map_err(failure)?;
                super::sync::project(node, &link, &project.town).await.map_err(failure)?;
            }
            path
        }
        _ => {
            let response = link
                .owned_records(
                    &asker.account,
                    fastctx_types::owned_records::Request::Resolve { conversation: owner.clone() },
                )
                .await
                .map_err(failure)?;
            let fastctx_types::owned_records::Response::Conversation { conversation } = response else {
                return Err(failure("conversation ownership was not returned"));
            };
            let view = crate::authorization::current_view(&link)?;
            if !view.me().is_some_and(|member| {
                fastctx_net::record_ownership::keeps(
                    &view.directory,
                    member,
                    std::slice::from_ref(&conversation.account),
                    conversation.shared_on.as_ref(),
                )
            }) {
                return Err(Failure::refused("this computer does not keep that conversation's records"));
            }
            let root = node.world.owner(&node.names(), &conversation.address);
            let known =
                node.replicas.state.folders.lock().expect("never poisoned").contains_key(&root.path().join("history"));
            if !known
                || (matches!(resource, Resource::History { .. })
                    && !fastctx_platform::disk::is_dir(root.path().join("history")))
            {
                super::sync::conversation(node, &link, conversation).await.map_err(failure)?;
            }
            if matches!(resource, Resource::History { .. }) {
                root.path().join("history")
            } else {
                root.path().to_path_buf()
            }
        }
    };
    let path = fastctx_platform::task::blocking(move || crate::world::native(&path)).await;
    Ok(Location { machine: node.machine(), path })
}

/// Candidate selection shared with context dependency tracking.
pub(crate) fn candidates(
    node: &Node,
    view: &fastctx_net::directory::View,
    account: &HubAccount,
    resource: &Resource,
    ownership: Option<&fastctx_types::owned_records::Conversation>,
    focus: &fastctx_types::focus::Focus,
    speaking: Option<&fastctx_types::MachineName>,
) -> Vec<Written> {
    let subject = Acting::one(account.hub.clone(), account.account.clone());
    selection::Selection { view, account, resource, ownership, focus, here: &node.machine(), speaking }
        .candidates(|machine| node.authorize_machine(&subject, machine).is_ok())
}
