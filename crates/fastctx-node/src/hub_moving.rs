//! Moving the hub, as the super administrator asks it from this computer
//! (`world-network.md`, 把中枢搬到别处). The hub decides and drives the
//! move; what only the machines themselves know, what they are and their
//! network addresses, this node asks each of them for.

use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::access::Purpose;
use fastctx_types::hub_move::{HubMoveReply, HubMoveRequest, MoveTarget};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::world::Form;
use fastctx_types::{AccountName, HubName};

use crate::Node;

pub async fn request(
    node: &Arc<Node>,
    hub: Option<&HubName>,
    account: &AccountName,
    request: HubMoveRequest,
) -> Result<HubMoveReply, Failure> {
    let link = crate::world::hub_link(node, hub)?;
    let targets = matches!(request, HubMoveRequest::Targets);
    let reply = link.hub_move(account, request).await.map_err(Failure::new)?;
    match reply {
        HubMoveReply::Targets { machines, port } if targets => {
            let mut asked = tokio::task::JoinSet::new();
            let count = machines.len();
            for (index, target) in machines.into_iter().enumerate() {
                let (link, account) = (link.clone(), account.clone());
                asked.spawn(async move { (index, described(&link, &account, target).await) });
            }
            let mut described = Vec::with_capacity(count);
            while let Some(done) = asked.join_next().await {
                described.push(done.map_err(|error| Failure::new(format!("asking the machines: {error}")))?);
            }
            described.sort_by_key(|(index, _)| *index);
            Ok(HubMoveReply::Targets { machines: described.into_iter().map(|(_, target)| target).collect(), port })
        }
        reply => Ok(reply),
    }
}

/// `target` with what the machine says of itself, when it is online: what
/// it is, and its network addresses. One that does not know how to be asked
/// for its addresses runs a fastctx too old to take the hub.
async fn described(link: &Link, account: &AccountName, mut target: MoveTarget) -> MoveTarget {
    if !target.online {
        return target;
    }
    let facts = link.call(&target.name, account, Purpose::Look, RemoteCall::Facts);
    let interfaces = link.call(&target.name, account, Purpose::Look, RemoteCall::Interfaces);
    let (facts, interfaces) = tokio::join!(facts, interfaces);
    if let Ok(Ok(RemoteReply::Facts(facts))) = facts {
        target.sleeps = facts.form == Some(Form::Laptop);
        target.os = Some(facts.os);
        target.form = facts.form;
    }
    let refused = match interfaces {
        Ok(Ok(RemoteReply::Interfaces { addresses })) => {
            target.interfaces = addresses;
            None
        }
        Ok(Ok(_)) => Some("it answered something else".to_string()),
        Ok(Err(failure)) => Some(format!("it runs a fastctx that cannot take the hub ({})", failure.message)),
        Err(error) => Some(error.to_string()),
    };
    if target.unavailable.is_none() {
        target.unavailable = refused;
    }
    if target.unavailable.is_none() && target.os.as_deref().is_some_and(|os| os != "windows" && os != "linux") {
        target.unavailable = Some("the hub runs on Windows and Linux".to_string());
    }
    target
}
