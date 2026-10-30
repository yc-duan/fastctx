//! Each document batch is a new admitted change, even on an old observation
//! connection. Losing that connection releases its place without abandoning SQL.

use super::*;
use fastctx_types::{
    ResidentId,
    composer::{
        Request,
        sync::{Frame, Input},
    },
    remote::RemoteCall,
};

pub(super) struct Incoming {
    pub input: Input,
    pub memory: fastctx_net::reassembly::Reservation,
    pub received: tokio::time::Instant,
}

pub(super) async fn run(
    node: Arc<Node>,
    link: Arc<Link>,
    caller: Caller,
    resident: ResidentId,
    incoming: Incoming,
    mut closed: tokio::sync::watch::Receiver<()>,
) -> Frame {
    let Input::Update { number, .. } = &incoming.input;
    let number = *number;
    let result = async {
        let call = RemoteCall::Composer { resident: resident.clone(), request: Request::Sync { input: Box::new(incoming.input) } };
        crate::admission::admit(&node, &link, &caller, &call)?;
        let admission = fastctx_net::link::admission::Request::call(caller.machine.clone(), caller.account.clone(), incoming.received, &call, &node.machine());
        let deadline = incoming.received + call.contract().within.expect("document changes have a deadline");
        let permit = tokio::select! {
            permit = crate::intake::take(&node, link.hub().clone(), admission) => permit?,
            _ = closed.changed() => return Err(Failure::not_run("the document subscription closed before this update began")),
        };
        crate::admission::admit(&node, &link, &caller, &call)?;
        let handover = node.handoff.call().ok_or_else(|| Failure::not_run("the node is handing over; this update was not started"))?;
        if tokio::time::Instant::now() >= deadline || closed.has_changed().is_err() {
            return Err(Failure::not_run("the document update was not started before its caller left or its time ran out"));
        }
        let RemoteCall::Composer { request: Request::Sync { input }, .. } = call else { unreachable!() };
        permit.started();
        let working = permit.clone();
        let worker_node = node.clone();
        let mut worker = tokio::spawn(crate::gate_waits::asked(async move {
            let _handover = handover;
            let _permit = working;
            let _memory = incoming.memory;
            let _doing = fastctx_platform::doing::doing(format!("updating {resident}'s shared document"));
            crate::composer::update_document(&worker_node, &resident, *input).await
        }));
        let reason = tokio::select! {
            result = &mut worker => return result.map_err(|error| Failure::new(error.to_string()))?,
            _ = closed.changed() => "the document connection closed; this update may still commit",
            () = tokio::time::sleep_until(deadline) => "the document update did not finish within 300 s; it is still being done there",
        };
        permit.settling();
        tokio::spawn(async move {
            match worker.await {
                Ok(Ok(_)) => {},
                Ok(Err(error)) => node.log.line(format_args!("finishing a shared document update: {}", error.message)),
                Err(error) => node.log.line(format_args!("finishing a shared document update: {error}")),
            }
        });
        Err(Failure::new(reason))
    }.await;
    result.unwrap_or_else(|failure| Frame::Refused { number, failure })
}
