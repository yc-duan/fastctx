//! Asks another machine for a record change under one name: again, while
//! that machine only says it accepted the work (`crate::submitted::Asking`).
use crate::Node;
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, MachineName,
    access::{Acting, Purpose, Written},
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    submission::Submission,
};
use std::sync::Arc;

/// The name the asking goes under: the one the work it is part of has, or
/// one of its own.
fn name() -> Result<Submission, Failure> {
    if let Some(submission) = fastctx_net::submission::current() {
        return Ok(submission);
    }
    let name = fastctx_net::sign::random::<16>().map_err(|error| Failure::new(error.to_string()))?;
    Ok(Submission { name: name.iter().map(|byte| format!("{byte:02x}")).collect(), waits_ms: u32::MAX })
}

pub(crate) async fn linked(
    link: &Arc<Link>,
    to: &MachineName,
    account: &AccountName,
    purpose: Purpose,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    fastctx_net::submission::scope(Some(name()?), async {
        let asking = crate::submitted::Asking::begin();
        loop {
            let reply =
                link.call(to, account, purpose, call.clone()).await.map_err(|error| Failure::new(error.to_string()))?;
            if let Some(reply) = asking.settled(reply).await {
                return reply;
            }
        }
    })
    .await
}

pub(crate) async fn on(
    node: &Arc<Node>,
    acting: &Acting,
    to: &Written,
    purpose: Purpose,
    call: RemoteCall,
) -> Result<RemoteReply, Failure> {
    fastctx_net::submission::scope(Some(name()?), async {
        let asking = crate::submitted::Asking::begin();
        loop {
            let reply =
                node.call(acting, to, purpose, call.clone()).await.map_err(|error| Failure::new(error.to_string()))?;
            if let Some(reply) = asking.settled(reply).await {
                return reply;
            }
        }
    })
    .await
}
