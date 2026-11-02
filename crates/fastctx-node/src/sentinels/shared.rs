//! Shared controls travel through the conversation's host without sharing account settings or keys.

use super::transport::Transport;
use crate::{Node, town::caller::Caller};
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, Address,
    access::HubAccount,
    ipc::Failure,
    owned_records,
    sentinel_control::{HomeReply, HomeRequest, Phase, Registration, Reply, Request, Summary},
};
use std::sync::Arc;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

async fn registrations(link: &Link, actor: &AccountName) -> Result<Vec<Registration>, Failure> {
    match link.owned_records(actor, owned_records::Request::Sentinels).await.map_err(failure)? {
        owned_records::Response::Sentinels { registrations } => Ok(registrations),
        _ => Err(failure("the hub did not return the visible sentinels")),
    }
}

pub(super) async fn route(
    node: &Arc<Node>,
    caller: &Caller,
    link: &Arc<Link>,
    actor: &AccountName,
    request: Request,
) -> Result<Reply, Failure> {
    if let Request::Conversation { owner, request } = request {
        return host(node, link, actor, owner, *request).await;
    }
    let registrations = registrations(link, actor).await?;
    match request {
        Request::List { .. } | Request::Overview => {
            let overview = matches!(request, Request::Overview);
            let owner = match &request {
                Request::List { owner } => owner.as_ref(),
                _ => None,
            };
            let mut owners = std::collections::BTreeSet::new();
            let mut sentinels = Vec::new();
            let mut watched = Vec::new();
            let mut unavailable = Vec::new();
            for registration in &registrations {
                if !overview && !registration.active {
                    continue;
                }
                let address = &registration.definition.owner;
                if owner.is_some_and(|owner| {
                    owner.resident != address.resident || !link.view().same(&owner.machine, &address.machine)
                }) || !owners.insert(address.clone())
                {
                    continue;
                }
                let request = if overview { Request::Overview } else { Request::List { owner: Some(address.clone()) } };
                match forward(node, link, actor, address, request).await {
                    Ok(Reply::Listed { sentinels: found, unavailable: errors }) => {
                        sentinels.extend(found);
                        unavailable.extend(errors);
                    }
                    Ok(Reply::Overview { watched: found, unavailable: errors }) => {
                        watched.extend(found);
                        unavailable.extend(errors);
                    }
                    Ok(_) => return Err(failure("the conversation host returned another sentinel reply")),
                    Err(error) => {
                        unavailable.push(error.message.clone());
                        if !overview {
                            for registration in registrations
                                .iter()
                                .filter(|candidate| candidate.active && candidate.definition.owner == *address)
                            {
                                sentinels.push(Summary {
                                    definition: registration.definition.clone(),
                                    phase: Phase::Unavailable,
                                    plan: String::new(),
                                    latest: None,
                                    reason: Some(error.message.clone()),
                                });
                            }
                        }
                    }
                }
            }
            if overview {
                Ok(Reply::Overview { watched, unavailable })
            } else {
                Ok(Reply::Listed { sentinels, unavailable })
            }
        }
        Request::Stop { ref id, .. } | Request::Record { ref id, .. } => {
            let registration = registrations
                .iter()
                .find(|registration| &registration.definition.id == id)
                .ok_or_else(|| failure("sentinel not found in this account's visible conversations"))?;
            if matches!(request, Request::Stop { .. })
                && let Caller::Resident(owner) = caller
                && (owner.resident != registration.definition.owner.resident
                    || !link.view().same(&owner.machine, &registration.definition.owner.machine))
            {
                return Err(Failure::refused("this conversation does not own the sentinel"));
            }
            forward(node, link, actor, &registration.definition.owner, request).await
        }
        _ => Err(Failure::refused("this operation is not a shared sentinel control")),
    }
}

async fn forward(
    node: &Arc<Node>,
    link: &Arc<Link>,
    actor: &AccountName,
    owner: &Address,
    request: Request,
) -> Result<Reply, Failure> {
    if node.is_me(&owner.machine) {
        return host(node, link, actor, owner.clone(), request).await;
    }
    let call = fastctx_types::remote::RemoteCall::SentinelControl {
        request: Request::Conversation { owner: owner.clone(), request: Box::new(request) },
    };
    match link.call(&owner.machine, actor, call.purpose(), call).await.map_err(failure)?? {
        fastctx_types::remote::RemoteReply::SentinelControl(reply) => Ok(*reply),
        _ => Err(failure("update the conversation host to manage shared sentinels")),
    }
}

async fn host(
    node: &Arc<Node>,
    link: &Arc<Link>,
    actor: &AccountName,
    owner: Address,
    request: Request,
) -> Result<Reply, Failure> {
    let owned_records::Response::Conversation { conversation } = link
        .owned_records(actor, owned_records::Request::Resolve { conversation: owner.clone() })
        .await
        .map_err(failure)?
    else {
        return Err(failure("the conversation ownership was not returned"));
    };
    if !node.is_me(&conversation.address.machine) {
        return Err(Failure::refused("ask the computer that runs this conversation"));
    }
    let scope = HubAccount { hub: link.membership().id, account: conversation.account.clone() };
    let access = crate::record_access::Access {
        actor: HubAccount { hub: scope.hub.clone(), account: actor.clone() },
        owner: scope.clone(),
        conversation: conversation.address.clone(),
    };
    let owned_records::Response::Sentinels { registrations } =
        link.owned_records(actor, owned_records::Request::Sentinels).await.map_err(failure)?
    else {
        return Err(failure("the hub did not return the visible sentinels"));
    };
    let selected: Vec<_> = registrations
        .into_iter()
        .filter(|registration| {
            registration.scope == scope && registration.definition.owner.resident == conversation.address.resident
        })
        .collect();
    let result = crate::record_access::within(Some(access), async {
        let records = node.sentinels.records()?;
        let homes = Transport(node.clone());
        match request {
            Request::List { .. } => fastctx_sentinel::lifecycle::list_where(records, &homes, &scope, &|candidate| {
                candidate.resident == conversation.address.resident
            })
            .await
            .map(|sentinels| Reply::Listed { sentinels, unavailable: Vec::new() })
            .map_err(failure),
            Request::Stop { id, home } => {
                let registration = selected
                    .iter()
                    .find(|registration| registration.definition.id == id && registration.active)
                    .ok_or_else(|| failure("sentinel not found"))?;
                if home.as_ref().is_some_and(|home| !link.view().same(home, &registration.definition.home)) {
                    return Err(failure("the sentinel lives on a different machine"));
                }
                let reason =
                    fastctx_sentinel::lifecycle::stop(records, &homes, &scope, &id, None).await.map_err(failure)?;
                Ok(Reply::Stopped { id, home_confirmed: reason.is_none(), reason })
            }
            Request::Record { id, home, generation, held } => {
                if !selected.iter().any(|registration| {
                    registration.definition.id == id
                        && link.view().same(&home, &registration.definition.home)
                        && registration.definition.generation >= generation
                }) {
                    return Err(Failure::refused("this sentinel record belongs to another conversation or generation"));
                }
                match homes
                    .send_to(&scope, &home, HomeRequest::Record { id, generation, held })
                    .await
                    .map_err(failure)?
                {
                    HomeReply::Record { record } => Ok(Reply::Record { record }),
                    _ => Err(failure("the sentinel home returned another reply")),
                }
            }
            Request::Overview => {
                let mut watched = Vec::new();
                let mut unavailable = Vec::new();
                let homes_to_ask: std::collections::BTreeSet<_> =
                    selected.iter().map(|registration| registration.definition.home.clone()).collect();
                for home in homes_to_ask {
                    match homes.send_to(&scope, &home, HomeRequest::Overview).await {
                        Ok(HomeReply::Overview { watched: found }) => watched.extend(found),
                        Ok(_) => return Err(failure("the sentinel home returned another reply")),
                        Err(error) => unavailable.push(error),
                    }
                }
                Ok(Reply::Overview { watched, unavailable })
            }
            _ => Err(Failure::refused("this operation is not a shared sentinel control")),
        }
    })
    .await?;
    link.owned_records(actor, owned_records::Request::Resolve { conversation: owner }).await.map_err(failure)?;
    Ok(result)
}
