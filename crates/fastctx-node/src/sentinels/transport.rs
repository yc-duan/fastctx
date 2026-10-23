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
impl Homes for Transport {
    fn begin<'a>(&'a self, registration: &'a Registration, version: u64) -> FutureResult<'a, Summary> {
        Box::pin(async move {
            match self
                .send(registration, HomeRequest::Begin { id: registration.definition.id.clone(), version, owner: registration.definition.owner.clone() })
                .await?
            {
                HomeReply::Summary { sentinel } if sentinel.definition == registration.definition => Ok(*sentinel),
                _ => Err("the sentinel home answered a different assignment".into()),
            }
        })
    }
    fn inspect<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, Summary> {
        Box::pin(async move {
            match self.send(registration, HomeRequest::Inspect { id: registration.definition.id.clone() }).await? {
                HomeReply::Summary { sentinel } if sentinel.definition == registration.definition => Ok(*sentinel),
                _ => Err("the sentinel home answered a different assignment".into()),
            }
        })
    }
    fn withdraw<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, ()> {
        Box::pin(async move {
            match self.send(registration, HomeRequest::Withdraw { id: registration.definition.id.clone() }).await? {
                HomeReply::Withdrawn => Ok(()),
                _ => Err("the sentinel home did not confirm withdrawal".into()),
            }
        })
    }
}
impl Transport {
    pub(super) async fn send(&self, registration: &Registration, request: HomeRequest) -> Result<HomeReply, String> {
        let link = self.0.link_of(&registration.scope.hub).ok_or("the sentinel account's network is not connected")?;
        let home = &registration.definition.home;
        let view = crate::authorization::current_view(&link).map_err(|e| e.message)?;
        if !view.call_rights(&view.me, &registration.scope.account, home).hands {
            return Err("this account may not use the sentinel home".into());
        }
        if view.is_me(home) {
            return home_call(&self.0, &registration.scope, &request).await;
        }
        if !view.member(home).is_some_and(|member| member.online) {
            return Err("the sentinel home is offline".into());
        }
        match link
            .call(home, &registration.scope.account, fastctx_types::access::Purpose::Look, RemoteCall::Facts)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.message)?
        {
            RemoteReply::Facts(facts) if facts.sentinels => {}
            _ => return Err("upgrade the sentinel home to use sentinels".into()),
        }
        let call = RemoteCall::SentinelHome { request };
        match link
            .call(home, &registration.scope.account, call.purpose(), call)
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.message)?
        {
            RemoteReply::SentinelHome(reply) => Ok(*reply),
            _ => Err("the sentinel home answered another kind of call".into()),
        }
    }
}
pub(crate) async fn home_call(
    node: &Arc<Node>,
    scope: &HubAccount,
    request: &HomeRequest,
) -> Result<HomeReply, String> {
    let host = super::worker::engine(node)?;
    host.call(scope, request).await
}
