//! Integration points for the goal engine; the engine owns state and notification.
use super::{control, transport::Transport};
use crate::{Node, town::caller::Caller};
use fastctx_net::link::Link;
use fastctx_types::{
    Address, SentinelId,
    access::HubAccount,
    ipc::Failure,
    sentinel::{Definition, GoalReview, Judgment, Kind},
    sentinel_control::{GoalRequest, HomeReply, HomeRequest, Registration},
    sentinel_settings::HomeStatus,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn owner_link(node: &Arc<Node>, scope: &HubAccount, owner: &Address) -> Result<Arc<Link>, Failure> {
    if !node.is_me(&owner.machine) {
        return Err(Failure::refused("goal sentinel requests originate on the owner's machine"));
    }
    node.residents.active(&owner.resident)?;
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::not_run("the goal account's hub is not connected"))?;
    let (selected, _) =
        control::scope(node, &Caller::Resident(owner.clone()), Some(&link.hub_name()), Some(&scope.account))?;
    Ok(selected)
}
fn id(link: &Link, scope: &HubAccount, owner: &Address) -> Result<SentinelId, Failure> {
    let view = crate::authorization::current_view(link)?;
    let member =
        view.member(&owner.machine).ok_or_else(|| Failure::not_run("the goal owner is not in the account's view"))?;
    let bytes =
        serde_json::to_vec(&("goal", scope, &member.key, &owner.resident)).map_err(|e| Failure::new(e.to_string()))?;
    format!("s-{}", &format!("{:x}", Sha256::digest(bytes))[..16])
        .parse()
        .map_err(|e: fastctx_types::NameError| Failure::new(e.to_string()))
}
/// Called after the goal engine durably opens or changes a goal. Retain the
/// returned assignment for judge/end; `reason` means it is registered but away.
pub(crate) async fn open(
    node: &Arc<Node>,
    scope: &HubAccount,
    owner: &Address,
    goal: &str,
) -> Result<(Registration, Option<String>), Failure> {
    let link = owner_link(node, scope, owner)?;
    let records = node.sentinels.records()?;
    let (_, settings) =
        fastctx_sentinel::settings::read(records, scope).await.map_err(|e| Failure::not_run(e.to_string()))?;
    let home = settings.home.ok_or_else(|| Failure::not_run("no sentinel home is configured"))?;
    let id = id(&link, scope, owner)?;
    let prior = fastctx_sentinel::lifecycle::get(records, scope, &id).await.map_err(|e| Failure::new(e.to_string()))?;
    let mut owner = owner.clone();
    if let Some((_, prior)) = &prior {
        let view = crate::authorization::current_view(&link)?;
        if prior.definition.owner.resident != owner.resident
            || !view.same(&prior.definition.owner.machine, &owner.machine)
        {
            return Err(Failure::refused("goal sentinel owner does not match"));
        }
        owner = prior.definition.owner.clone();
    }
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| Failure::new(e.to_string()))?;
    let registration = Registration {
        scope: scope.clone(),
        active: true,
        definition: Definition {
            id,
            hub: link.hub_name(),
            account: scope.account.clone(),
            owner,
            home,
            generation: u64::from_le_bytes(bytes),
            condition: goal.into(),
            at: Vec::new(),
            kind: Kind::Goal,
            tell: String::new(),
        },
    };
    fastctx_sentinel::goal::open(records, &Transport(node.clone()), registration)
        .await
        .map_err(|e| Failure::new(e.to_string()))
}
/// Stable request IDs deduplicate completed judgments, including after restart.
/// The caller must recheck its current goal/idle/wait state before using advice.
pub(crate) async fn judge(
    node: &Arc<Node>,
    registration: &Registration,
    request_id: &str,
    review: GoalReview,
) -> Judgment {
    let result = async {
        let link = owner_link(node, &registration.scope, &registration.definition.owner)?;
        if super::settings::home_status(&link, &registration.scope.account, Some(&registration.definition.home)).await
            != HomeStatus::Ready
        {
            return Ok(Judgment::Unavailable { reason: "the goal sentinel home is unavailable".into() });
        }
        let request = GoalRequest {
            id: registration.definition.id.clone(),
            generation: registration.definition.generation,
            owner: registration.definition.owner.clone(),
            request_id: request_id.into(),
            review,
        };
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(240),
            Transport(node.clone()).send(registration, HomeRequest::Judge { request: Box::new(request) }),
        )
        .await
        .map_err(|_| Failure::new("the goal sentinel did not answer in time"))?
        .map_err(Failure::new)?;
        match reply {
            HomeReply::Judged { judgment } => Ok(judgment),
            _ => Err(Failure::new("the goal sentinel answered another kind of call")),
        }
    }
    .await;
    result.unwrap_or_else(|e: Failure| Judgment::Unavailable { reason: e.message })
}
/// Called for a cleared/completed/removed goal. A stale assignment is refused.
pub(crate) async fn end(node: &Arc<Node>, registration: &Registration) -> Result<Option<String>, Failure> {
    // Removal may already have made the resident inactive. Account authority is
    // retained by the engine; deleting its record never starts another turn.
    if !node.is_me(&registration.definition.owner.machine) {
        return Err(Failure::refused("goal owner is on another machine"));
    }
    fastctx_sentinel::goal::end(node.sentinels.records()?, &Transport(node.clone()), registration)
        .await
        .map_err(|e| Failure::new(e.to_string()))
}
/// Ends the sentinel of `owner`'s goal on `scope`, where there is one.
pub(crate) async fn retire(node: &Arc<Node>, scope: &HubAccount, owner: &Address) -> Result<(), Failure> {
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::not_run("the goal account's hub is not connected"))?;
    let id = id(&link, scope, owner)?;
    let records = node.sentinels.records()?;
    let Some((_, registration)) =
        fastctx_sentinel::lifecycle::get(records, scope, &id).await.map_err(|e| Failure::new(e.to_string()))?
    else {
        return Ok(());
    };
    end(node, &registration).await.map(|_| ())
}
/// Definitions for the existing turn-evidence SentinelWaits adapter. Errors must
/// remain errors; an unavailable account cannot be presented as no background work.
pub(crate) async fn waiting(node: &Arc<Node>, owner: &Address) -> Result<Vec<Registration>, Failure> {
    let mut result = Vec::new();
    let scopes = control::scopes(node, &Caller::Resident(owner.clone()), None, None);
    if scopes.is_empty() {
        return Err(Failure::not_run("no current account view for sentinel waits"));
    }
    for (link, account) in scopes {
        let scope = HubAccount { hub: link.membership().id.clone(), account };
        let view = crate::authorization::current_view(&link)?;
        result.extend(
            fastctx_sentinel::goal::waiting(node.sentinels.records()?, &scope)
                .await
                .map_err(|e| Failure::new(e.to_string()))?
                .into_iter()
                .filter(|r| {
                    r.definition.owner.resident == owner.resident
                        && view.same(&r.definition.owner.machine, &owner.machine)
                }),
        );
    }
    Ok(result)
}
