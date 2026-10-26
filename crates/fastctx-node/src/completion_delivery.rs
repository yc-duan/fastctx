//! The conversation's decision about a completion. No conversation gate or
//! machine lock spans the follower query or the wait before another attempt.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use fastctx_net::link::CallFailure;
use fastctx_types::access::Purpose;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::{Address, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Progress {
    failures: u32,
    ceiling_tries: u32,
    pub(crate) due: Option<Timestamp>,
    reason: Option<String>,
}

pub(crate) enum Decision {
    Consumed,
    Held,
    Deliver { unconfirmed: bool },
}

pub(crate) async fn decide(
    node: &Arc<Node>,
    to: &ResidentId,
    delivery: &Delivery,
    progress: &mut Progress,
) -> io::Result<Decision> {
    let Notice::JobEnded { machine, job, request, follower, .. } = &delivery.notice else {
        return Err(io::Error::other("completion decision requested for another kind of letter"));
    };
    let directory = node.world.conversation(to);
    let ids = crate::on_delivery::completion_ids(node, to, delivery)?;
    for id in &ids {
        if crate::inbox::delivered_here(&directory, id)? {
            return Ok(Decision::Consumed);
        }
    }
    let owner = Address { machine: node.machine(), resident: to.clone() };
    let (taken, following) = crate::on_delivery::status(node, &owner, machine, job, request.as_ref())?;
    if taken || crate::on_delivery::stopped(node, to, machine, request.as_ref())? {
        for id in &ids {
            crate::inbox::consume(&directory, id)?;
        }
        return Ok(Decision::Consumed);
    }
    let Some(follower) = follower.as_ref().filter(|follower| !node.is_me(follower)) else {
        progress.due = None;
        return Ok(if following { Decision::Held } else { Decision::Deliver { unconfirmed: false } });
    };
    let reachable = node
        .locate(&follower.into())
        .is_ok_and(|links| links.iter().any(|link| link.view().member(follower).is_some_and(|member| member.online)));
    if !reachable {
        return Ok(Decision::Deliver { unconfirmed: true });
    }
    if progress.due.is_some_and(|due| due > Timestamp::now()) {
        return Ok(Decision::Held);
    }
    let acting = node.residents.about(to).map(|(about, _)| about.acting).unwrap_or_default();
    let call = RemoteCall::OnReceipt { owner, machine: machine.clone(), job: job.clone(), request: request.clone() };
    let result =
        tokio::time::timeout(Duration::from_secs(10), node.call(&acting, &follower.into(), Purpose::Hands, call)).await;
    let reason = match result {
        Ok(Ok(Ok(RemoteReply::OnReceipt { taken: true, .. }))) => {
            for id in &ids {
                crate::inbox::consume(&directory, id)?;
            }
            return Ok(Decision::Consumed);
        }
        Ok(Ok(Ok(RemoteReply::OnReceipt { taken: false, following: false }))) => {
            return Ok(Decision::Deliver { unconfirmed: false });
        }
        Ok(Ok(Ok(RemoteReply::OnReceipt { .. }))) => "its CLI is still following the end".to_string(),
        Ok(Ok(Ok(_))) => return Ok(Decision::Deliver { unconfirmed: true }),
        Ok(Ok(Err(error))) if refused_for_good(&error) => return Ok(Decision::Deliver { unconfirmed: true }),
        Ok(Ok(Err(error))) => error.message,
        Ok(Err(
            CallFailure::Offline { .. }
            | CallFailure::NotMember { .. }
            | CallFailure::NotAllowed { .. }
            | CallFailure::Refused { .. }
            | CallFailure::Unreadable { .. },
        )) => {
            return Ok(Decision::Deliver { unconfirmed: true });
        }
        Ok(Err(error)) => error.to_string(),
        Err(_) => "the follower did not answer within 10 seconds".to_string(),
    };
    if fastctx_net::again::at_ceiling(progress.failures) {
        progress.ceiling_tries = progress.ceiling_tries.saturating_add(1);
        if progress.ceiling_tries >= 2 {
            return Ok(Decision::Deliver { unconfirmed: true });
        }
    }
    if progress.reason.as_ref() != Some(&reason) {
        node.log.line(format_args!("completion {} for {to}: waiting to ask {follower}: {reason}", delivery.id));
        progress.reason = Some(reason);
    }
    progress.failures = progress.failures.saturating_add(1);
    progress.due =
        Some(Timestamp::now().checked_add(fastctx_net::again::backoff(progress.failures)).map_err(io::Error::other)?);
    Ok(Decision::Held)
}

/// The follower refused the question for good: asking again is refused the
/// same way. Nothing done is not enough, as a node handing over or too busy
/// to answer now did nothing either, and answers once asked again.
fn refused_for_good(error: &fastctx_types::ipc::Failure) -> bool {
    error.is_refused() && error.denial != Some(fastctx_types::authorization::DenialReason::ViewUnavailable)
}
