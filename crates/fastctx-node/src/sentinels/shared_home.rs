//! A sentinel home answers a shared conversation through its authenticated runtime host.

use crate::Node;
use fastctx_agent::FutureResult;
use fastctx_sentinel::ports::ResidentHost;
use fastctx_types::{
    AccountName, Address, MachineName,
    access::HubAccount,
    account_records, owned_records,
    sentinel_control::{HomeReply, HomeRequest, Registration},
};
use std::sync::Arc;

fn authorized<'a>(
    link: &'a fastctx_net::link::Link,
    scope: &'a HubAccount,
    from: &'a MachineName,
    actor: &'a AccountName,
    owner: &'a Address,
) -> FutureResult<'a, ()> {
    Box::pin(async move {
        let owned_records::Response::Conversation { conversation } =
            link.owned_records(&scope.account, owned_records::Request::Resolve { conversation: owner.clone() }).await?
        else {
            return Err("the conversation ownership was not returned".into());
        };
        if conversation.account != scope.account || !link.view().same(&conversation.address.machine, from) {
            return Err("only the conversation's runtime host may forward its shared sentinel controls".into());
        }
        match link
            .owned_records(
                &scope.account,
                owned_records::Request::Authorize { conversation: owner.clone(), reader: actor.clone() },
            )
            .await?
        {
            owned_records::Response::Authorization { allowed: true } => Ok(()),
            _ => Err("the acting account no longer owns this conversation".into()),
        }
    })
}

pub(super) async fn call(
    node: &Arc<Node>,
    scope: &HubAccount,
    from: &MachineName,
    actor: &AccountName,
    owner: &Address,
    request: &HomeRequest,
) -> Result<HomeReply, String> {
    let link = node.link_of(&scope.hub).ok_or("the sentinel's hub is unavailable")?;
    authorized(&link, scope, from, actor, owner).await?;
    let id = match request {
        HomeRequest::Inspect { id } | HomeRequest::Withdraw { id } | HomeRequest::Record { id, .. } => Some(id),
        HomeRequest::Overview => None,
        _ => return Err("this sentinel operation cannot be shared".into()),
    };
    if let Some(id) = id {
        let account_records::Response::Read { record: Some(record) } = link
            .account_records(
                &scope.account,
                account_records::Request::Read { kind: "sentinel".into(), key: id.to_string() },
            )
            .await?
        else {
            return Err("the sentinel's ownership is unavailable".into());
        };
        let registration: Registration = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
        if registration.scope != *scope
            || registration.definition.owner.resident != owner.resident
            || registration.definition.id != *id
        {
            return Err("this sentinel belongs to another conversation".into());
        }
    }
    let host = super::worker::engine(node)?;
    let request = match request {
        HomeRequest::Record { id, generation, .. } => {
            HomeRequest::Record { id: id.clone(), generation: *generation, held: None }
        }
        request => request.clone(),
    };
    let mut reply = crate::record_access::within(None, host.call(scope, &request)).await?;
    let owns = |definition: &fastctx_types::sentinel::Definition| {
        definition.account == scope.account && definition.owner.resident == owner.resident
    };
    match &mut reply {
        HomeReply::Summary { sentinel } if !owns(&sentinel.definition) => {
            return Err("the sentinel's stored assignment belongs to another conversation".into());
        }
        HomeReply::Record { record } if record.summary.as_ref().is_none_or(|summary| !owns(&summary.definition)) => {
            return Err("the sentinel's stored generation belongs to another conversation".into());
        }
        HomeReply::Overview { watched } => watched.retain(|watched| owns(&watched.summary.definition)),
        _ => {}
    }
    authorized(&link, scope, from, actor, owner).await?;
    Ok(reply)
}
