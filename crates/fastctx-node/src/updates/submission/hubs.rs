//! Effectful hub requests retain their accepted hub. Reads keep the direct path.
//! Installation invitations remain outside persisted operations.
use super::*;
use fastctx_types::{
    HubName,
    hub_move::{HubMoveReply, HubMoveRequest},
    update::{ViewCall, ViewReply},
};

const NETWORK: Family = Family {
    name: "call/network-updates",
    at_once: 8,
    within: Duration::from_secs(15 * 60),
    stays: Duration::from_secs(45),
    again: false,
};
const MOVING: Family = Family {
    name: "call/hub-move",
    at_once: 4,
    within: Duration::from_secs(20 * 60),
    stays: Duration::from_secs(45),
    again: false,
};
const ARRIVED: Family = Family {
    name: "call/hub-arrived",
    at_once: 4,
    within: Duration::from_secs(5 * 60),
    stays: Duration::from_secs(45),
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Network {
    hub: HubName,
    account: AccountName,
    request: ViewCall,
}
#[derive(Serialize, Deserialize)]
struct Moving {
    hub: Option<HubName>,
    account: AccountName,
    request: HubMoveRequest,
}
#[derive(Serialize, Deserialize)]
struct Arrived {
    hub: HubId,
}

#[derive(Serialize, Deserialize)]
struct Answer<T> {
    hub: HubId,
    result: Result<T, Failure>,
}

pub(super) fn register(node: &Arc<Node>) {
    submitted::register_resolved(node, NETWORK, |node: Arc<Node>, asked: Network, hub: HubId| async move {
        let result = match link(&node, &hub) {
            Ok(link) => link.network_updates(&asked.account, asked.request).await.map_err(Failure::new),
            Err(failure) => Err(failure),
        };
        Ok::<_, Failure>(Answer { hub, result })
    });
    submitted::register_resolved(node, MOVING, |node: Arc<Node>, asked: Moving, hub: HubId| async move {
        crate::hub_moving::request(&link(&node, &hub)?, &asked.account, asked.request).await
    });
    submitted::register(node, ARRIVED, |node: Arc<Node>, asked: Arrived| async move {
        crate::hub_arrival::installed(&node, &asked.hub).await
    });
}

pub(crate) async fn network(
    node: &Arc<Node>,
    hub: HubName,
    account: AccountName,
    request: ViewCall,
) -> Result<ViewReply, Failure> {
    if matches!(request, ViewCall::View | ViewCall::Release { .. }) {
        return crate::world::hub_link(node, Some(&hub))?
            .network_updates(&account, request)
            .await
            .map_err(Failure::new);
    }
    let asked = Network { hub, account, request };
    let answer: Answer<ViewReply> = submitted::submit_resolving(node, &NETWORK, &asked, || async {
        let link = crate::world::hub_link(node, Some(&asked.hub))?;
        if matches!(
            asked.request,
            ViewCall::Repair { .. } | ViewCall::CancelRepair { .. } | ViewCall::RepairResult { .. }
        ) && !link.versions().is_some_and(|network| network.repair_supported)
        {
            return Err(Failure::new("this hub is too old to arrange repairs; update the hub first"));
        }
        if !link.versions().is_some_and(|network| network.updates_view) {
            return Err(Failure::new("this hub does not support the updates view; update the hub first"));
        }
        let hub = link.membership().id.clone();
        network_authorized(node, &hub, &asked.account)?;
        Ok((super::unit("hub", &hub), hub))
    })
    .await?;
    network_authorized(node, &answer.hub, &asked.account)?;
    answer.result
}

pub(crate) async fn moving(
    node: &Arc<Node>,
    hub: Option<HubName>,
    account: AccountName,
    request: HubMoveRequest,
) -> Result<HubMoveReply, Failure> {
    if matches!(request, HubMoveRequest::Targets | HubMoveRequest::Status) {
        let link = crate::world::hub_link(node, hub.as_ref())?;
        return crate::hub_moving::request(&link, &account, request).await;
    }
    let asked = Moving { hub, account, request };
    // This is the outcome of the caller's own operation, including a move
    // after which the caller no longer has a membership in the old hub.
    submitted::submit_resolving(node, &MOVING, &asked, || async {
        let hub = prepare_moving(node, &asked)?;
        Ok((super::unit("hub", &hub), hub))
    })
    .await
}

fn prepare_moving(node: &Node, asked: &Moving) -> Result<HubId, Failure> {
    let link = crate::world::hub_link(node, asked.hub.as_ref())?;
    let view = crate::authorization::current_view(&link)?;
    if !view.logged_in(&view.me, &asked.account) {
        return Err(Failure::new(format!("{} is not logged in to the account {}", view.me, asked.account)));
    }
    if matches!(&asked.request, HubMoveRequest::Start { addresses, .. } if addresses.is_empty()) {
        return Err(Failure::new("give the address the hub is to be reached at there"));
    }
    // The hub keeps the authoritative super-administrator and move-stage checks.
    Ok(link.membership().id.clone())
}

fn network_authorized(node: &Node, hub: &HubId, account: &AccountName) -> Result<(), Failure> {
    let link = link(node, hub)?;
    let view = crate::authorization::current_view(&link)?;
    if !view.logged_in(&view.me, account) || view.account(account).is_none_or(|account| account.locked) {
        return Err(Failure::new(format!("{} is not logged in to an unlocked account {account}", view.me)));
    }
    Ok(())
}

fn link(node: &Node, hub: &HubId) -> Result<Arc<fastctx_net::link::Link>, Failure> {
    node.link_of(hub).ok_or_else(|| Failure::new(format!("this computer is not on the hub {hub}")))
}

pub(crate) async fn arrived(node: &Arc<Node>, hub: HubId) -> Result<(), Failure> {
    let asked = Arrived { hub };
    submitted::submit_resolving(node, &ARRIVED, &asked, || async {
        link(node, &asked.hub)?;
        Ok((super::unit("hub", &asked.hub), ()))
    })
    .await
}
