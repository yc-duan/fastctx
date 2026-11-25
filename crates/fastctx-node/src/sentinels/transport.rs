//! Sentinel calls travel as their authenticated account, never an invented resident.
use crate::Node;
use fastctx_agent::FutureResult;
use fastctx_sentinel::{lifecycle::Homes, ports::ResidentHost};
use fastctx_types::{
    access::HubAccount,
    remote::{RemoteCall, RemoteReply},
    sentinel_control::*,
};
use std::sync::Arc;

pub(crate) struct Transport(pub Arc<Node>);

/// A sentinel as asked of its home: its account, its home, its name.
pub(crate) type Asked = (HubAccount, fastctx_types::MachineName, fastctx_types::SentinelId);

/// What a home said of a sentinel, and the number its sentinels stood at
/// when it said it (`fastctx_types::numbers::Numbers::sentinels`).
#[derive(Clone)]
pub(crate) struct Inspected {
    number: u64,
    summary: Summary,
}

/// The most inspections kept: past it they are let go, all at once.
const INSPECTED_AT_MOST: usize = 4096;

impl Transport {
    /// The number the sentinels of `registration`'s home stand at, as its
    /// hub last told it, where it tells it, is online, and the sentinel's
    /// account may use it still (as `send_to` asks); none for this
    /// computer, which is asked at no cost, and none for a conversation's
    /// sharer, whom the home judges itself.
    fn standing(&self, registration: &Registration) -> Option<u64> {
        if crate::record_access::current().is_some() {
            return None;
        }
        let link = self.0.link_of(&registration.scope.hub)?;
        let view = link.view();
        let home = &registration.definition.home;
        if view.is_me(home) || !view.call_rights(&view.me, &registration.scope.account, home).hands {
            return None;
        }
        view.member(home).filter(|member| member.online).and_then(|member| member.numbers?.sentinels)
    }
}

impl Homes for Transport {
    fn begin<'a>(&'a self, registration: &'a Registration, version: u64) -> FutureResult<'a, Summary> {
        Box::pin(async move {
            match self
                .send(
                    registration,
                    HomeRequest::Begin {
                        id: registration.definition.id.clone(),
                        version,
                        owner: registration.definition.owner.clone(),
                    },
                )
                .await?
            {
                HomeReply::Summary { sentinel } if sentinel.definition == registration.definition => Ok(*sentinel),
                _ => Err("the sentinel home answered a different assignment".into()),
            }
        })
    }
    /// What its home says of the sentinel: as it said last, while the number
    /// its sentinels stand at has not moved since (`standing`).
    fn inspect<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, Summary> {
        Box::pin(async move {
            let asked: Asked =
                (registration.scope.clone(), registration.definition.home.clone(), registration.definition.id.clone());
            let standing = self.standing(registration);
            if let Some(number) = standing {
                let kept = self.0.sentinels.inspected.lock().expect("never poisoned").get(&asked).cloned();
                if let Some(kept) =
                    kept.filter(|kept| kept.number == number && kept.summary.definition == registration.definition)
                {
                    return Ok(kept.summary);
                }
            }
            let summary =
                match self.send(registration, HomeRequest::Inspect { id: registration.definition.id.clone() }).await? {
                    HomeReply::Summary { sentinel } if sentinel.definition == registration.definition => *sentinel,
                    _ => return Err("the sentinel home answered a different assignment".into()),
                };
            if let Some(number) = standing {
                let mut inspected = self.0.sentinels.inspected.lock().expect("never poisoned");
                if inspected.len() >= INSPECTED_AT_MOST {
                    inspected.clear();
                }
                inspected.insert(asked, Inspected { number, summary: summary.clone() });
            }
            Ok(summary)
        })
    }
    fn withdraw<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, ()> {
        Box::pin(async move {
            self.0.sentinels.inspected.lock().expect("never poisoned").remove(&(
                registration.scope.clone(),
                registration.definition.home.clone(),
                registration.definition.id.clone(),
            ));
            match self.send(registration, HomeRequest::Withdraw { id: registration.definition.id.clone() }).await? {
                HomeReply::Withdrawn => Ok(()),
                _ => Err("the sentinel home did not confirm withdrawal".into()),
            }
        })
    }
}
impl Transport {
    pub(super) async fn send(&self, registration: &Registration, request: HomeRequest) -> Result<HomeReply, String> {
        self.send_to(&registration.scope, &registration.definition.home, request).await
    }

    /// Sends `request` to the sentinel home `home` for the account `scope`:
    /// as to a sentinel's, whose account record may be gone with its end.
    pub(super) async fn send_to(
        &self,
        scope: &HubAccount,
        home: &fastctx_types::MachineName,
        request: HomeRequest,
    ) -> Result<HomeReply, String> {
        let request = if let Some(access) = crate::record_access::current() {
            if &access.owner != scope {
                return Err("a shared sentinel request changed its owning account".into());
            }
            if !matches!(
                request,
                HomeRequest::Inspect { .. }
                    | HomeRequest::Withdraw { .. }
                    | HomeRequest::Record { .. }
                    | HomeRequest::Overview
            ) {
                return Err("this sentinel operation cannot be shared".into());
            }
            HomeRequest::Conversation {
                actor: access.actor.account,
                owner: access.conversation,
                request: Box::new(request),
            }
        } else {
            request
        };
        let link = self.0.link_of(&scope.hub).ok_or("the sentinel account's network is not connected")?;
        let view = crate::authorization::current_view(&link).map_err(|e| e.message)?;
        if !view.call_rights(&view.me, &scope.account, home).hands {
            return Err("this account may not use the sentinel home".into());
        }
        if view.is_me(home) {
            return home_call_for(&self.0, scope, &self.0.machine(), &request).await.map_err(|failure| failure.message);
        }
        let Some(member) = view.member(home).filter(|member| member.online) else {
            return Err("the sentinel home is offline".into());
        };
        // Whether it keeps sentinels is asked once for each time it connects.
        let since = member.since;
        let able = (scope.hub.clone(), home.clone());
        let known =
            since.is_some() && self.0.sentinels.able.lock().expect("never poisoned").get(&able) == since.as_ref();
        if !known {
            match link
                .call(home, &scope.account, fastctx_types::access::Purpose::Look, RemoteCall::Facts)
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.message)?
            {
                RemoteReply::Facts(facts) if facts.sentinels => {
                    if let Some(since) = since {
                        self.0.sentinels.able.lock().expect("never poisoned").insert(able, since);
                    }
                }
                _ => return Err("upgrade the sentinel home to use sentinels".into()),
            }
        }
        let call = RemoteCall::SentinelHome { request };
        let asking = crate::submitted::Asking::begin();
        loop {
            match link.call(home, &scope.account, call.purpose(), call.clone()).await.map_err(|e| e.to_string())? {
                Ok(RemoteReply::SentinelHome(reply)) => return Ok(*reply),
                Ok(_) => return Err("the sentinel home answered another kind of call".into()),
                // Named submissions retain the same operation on each ask.
                Err(failure) => asking.accepted(failure).await.map_err(|failure| failure.message)?,
            }
        }
    }
}
pub(crate) async fn home_call_for(
    node: &Arc<Node>,
    scope: &HubAccount,
    from: &fastctx_types::MachineName,
    request: &HomeRequest,
) -> Result<HomeReply, fastctx_types::ipc::Failure> {
    request.validate_bounds().map_err(fastctx_types::ipc::Failure::not_run)?;
    if let HomeRequest::Judge { request } = request
        && (request.request_id.len() > 256 || request.request_id.trim().is_empty())
    {
        return Ok(HomeReply::Judged {
            judgment: fastctx_types::sentinel::Judgment::NotJudged { reason: "invalid goal review ID".into() },
        });
    }
    let changes = matches!(
        request,
        HomeRequest::Begin { .. } | HomeRequest::Judge { .. } | HomeRequest::Withdraw { .. }
    ) || matches!(request, HomeRequest::Conversation { request, .. } if matches!(request.as_ref(), HomeRequest::Withdraw { .. }));
    if changes {
        node.sentinels.records()?;
        return super::submission::home(node, scope, from, request).await;
    }
    home_work(node, scope, from, request).await.map_err(fastctx_types::ipc::Failure::new)
}

pub(super) async fn home_work(
    node: &Arc<Node>,
    scope: &HubAccount,
    from: &fastctx_types::MachineName,
    request: &HomeRequest,
) -> Result<HomeReply, String> {
    if let HomeRequest::Conversation { actor, owner, request } = request {
        return super::shared_home::call(node, scope, from, actor, owner, request).await;
    }
    let host = super::worker::engine(node)?;
    let id = match request {
        HomeRequest::Begin { id, .. } => Some(id),
        HomeRequest::Judge { request } => Some(&request.id),
        _ => None,
    };
    if let Some(id) = id {
        let registration = fastctx_sentinel::lifecycle::get(host.records.as_ref(), scope, id)
            .await
            .map_err(|error| error.to_string())?;
        let goal = registration.as_ref().is_some_and(|(_, r)| r.definition.kind == fastctx_types::sentinel::Kind::Goal);
        if !crate::account_features::sentinel(node, scope, goal).await.map_err(|error| error.message)? {
            return Err(if goal {
                fastctx_types::goals::words::DISABLED
            } else {
                fastctx_types::sentinel_words::DISABLED
            }
            .into());
        }
    }
    host.call(scope, request).await
}
