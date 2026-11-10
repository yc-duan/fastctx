//! Update forwarding keeps the accepted computer and network routes on every ask.
use super::*;
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::remote::{RemoteCall, RemoteResult};
use std::future::Future;

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum Target {
    Here,
    Remote { identity: String, hubs: Vec<HubId> },
    Pending { hubs: Vec<HubId> },
}

impl Target {
    pub(super) fn same_computer(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Here, Self::Here) => true,
            (Self::Remote { identity: a, .. }, Self::Remote { identity: b, .. }) => a == b,
            _ => false,
        }
    }
}

tokio::task_local! {
    static TARGET: Target;
}

pub(super) async fn within<T>(target: Target, work: impl Future<Output = T>) -> T {
    TARGET.scope(target, work).await
}

pub(super) fn target(node: &Node, machine: &Written) -> Result<Target, Failure> {
    if node.is_here(machine) {
        return Ok(Target::Here);
    }
    Ok(from_links(machine, &node.locate(machine)?))
}

fn from_links(machine: &Written, links: &[Arc<fastctx_net::link::Link>]) -> Target {
    let hubs = links.iter().map(|link| link.membership().id.clone()).collect();
    let key = links.iter().find_map(|link| link.view().member(&machine.name).map(|member| member.key));
    match key {
        Some(key) => Target::Remote { identity: super::unit("machine", &key), hubs },
        None => Target::Pending { hubs },
    }
}

pub(super) async fn resolve(node: &Node, machine: &Written, target: Target) -> Result<Target, Failure> {
    let hubs = match &target {
        Target::Here => return Ok(target),
        Target::Remote { hubs, .. } | Target::Pending { hubs } => hubs,
    };
    let links: Vec<_> = hubs.iter().filter_map(|hub| node.link_of(hub)).collect();
    for link in &links {
        crate::authorization::awaited_view(link).await?;
    }
    if !matches!(target, Target::Pending { .. }) {
        return Ok(target);
    }
    if node.is_here(machine) && links.iter().any(|link| link.view().is_me(&machine.name)) {
        return Ok(Target::Here);
    }
    match from_links(machine, &links) {
        Target::Pending { .. } => Err(Failure::not_run(format!("the identity of {machine} is not known yet"))),
        target => Ok(target),
    }
}

pub(crate) async fn call(
    node: &Arc<Node>,
    acting: &Acting,
    machine: &Written,
    purpose: Purpose,
    call: RemoteCall,
) -> Result<RemoteResult, Failure> {
    let expected = match TARGET.try_with(Clone::clone) {
        Ok(target) => target,
        Err(_) => return node.call(acting, machine, purpose, call).await.map_err(|why| Failure::new(why.to_string())),
    };
    let unavailable = || Failure::not_run(format!("the accepted route to {machine} is no longer available"));
    let Target::Remote { identity, hubs } = expected else { return Err(unavailable()) };
    node.settled(machine).await;
    let speakers = node.speakers(acting, machine, purpose).map_err(|why| Failure::new(why.to_string()))?;
    let mut refused = None;
    for (link, account) in speakers {
        if !hubs.contains(&link.membership().id) {
            continue;
        }
        let view = crate::authorization::awaited_view(&link).await?;
        if view.member(&machine.name).is_none_or(|member| super::unit("machine", &member.key) != identity) {
            continue;
        }
        // Keep this link across preparation and delivery; a newly joined hub
        // with the same machine name cannot receive a repeated accepted call.
        match link
            .call(&machine.name, &account, purpose, call.clone())
            .await
            .map_err(|why| Failure::new(why.to_string()))?
        {
            Err(failure) if failure.not_run => refused = Some(failure),
            answered => return Ok(answered),
        }
    }
    match refused {
        Some(failure) => Ok(Err(failure)),
        None => Err(unavailable()),
    }
}
