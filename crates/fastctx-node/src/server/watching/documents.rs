//! Native local subscriptions retain update permits through commit, even if the
//! observer leaves. Each queued input owns its global byte reservation.

use crate::Node;
use fastctx_net::{reassembly::Reservation, watch_queue};
use fastctx_types::{
    Call,
    composer::{
        Request,
        sync::{Frame, Input},
    },
    ipc::{Failure, Peer},
    session::Watched,
    watching::local::Subscription,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub(super) struct Incoming {
    pub input: Input,
    pub memory: Reservation,
}

pub(super) async fn open(
    node: &Arc<Node>,
    peer: &Peer,
    mut request: Subscription,
    mut incoming: mpsc::Receiver<Incoming>,
) -> Result<watch_queue::Receiver<Result<Watched, Failure>>, Failure> {
    let resident = request.resident.clone();
    let local = request.machine.is_none() && node.residents.about(&resident).is_some();
    let machine = match request.machine.clone() {
        Some(machine) => machine,
        None => crate::talk::locate(node, &resident).await?,
    };
    let subscription = request.documents.take().expect("native subscription");
    let name = request
        .composer
        .take()
        .ok_or_else(|| Failure::not_run("document synchronization requires a watch identity"))?;
    let (sender, receiver) = watch_queue::channel(|piece: &Result<Watched, Failure>| match piece {
        Ok(piece) => watch_queue::watched_size(piece),
        Err(error) => (error.message.len(), None),
    });
    if local || node.is_here(&machine) {
        let mut documents = crate::composer::watch_documents(node, &resident, name, subscription).await?;
        let (node, peer) = (node.clone(), peer.clone());
        fastctx_platform::task::spawn(async move {
            let mut updating = None;
            loop {
                let result = tokio::select! {
                    () = sender.closed() => return,
                    frame = documents.next() => frame.map(|frame| frame.map(Arc::unwrap_or_clone)),
                    Some(input) = incoming.recv(), if updating.is_none() => {
                        let (node, peer, resident, machine) = (node.clone(), peer.clone(), resident.clone(), request.machine.clone());
                        // Dropping the JoinHandle detaches accepted SQL; the commit
                        // callback still publishes it after the observer disconnects.
                        updating = Some(fastctx_platform::task::spawn(crate::gate_waits::asked(async move {
                            let Incoming { input, memory: _memory } = input;
                            let number = input.number();
                            let result = async {
                                crate::composer::validate_document_input(&input)?;
                                super::super::users_voice(&node, &peer)?;
                                let request = match input {
                                    Input::Mutate { mutation, .. } => Request::Mutate { mutation },
                                    input => Request::Sync { input: Box::new(input) },
                                };
                                let call = Call::Composer { resident: resident.clone(), machine, request };
                                crate::admission::local::admit(&node, &peer, &call)?;
                                let _handover = node.handoff.call().ok_or_else(|| Failure::not_run("the node is handing over; this update was not started"))?;
                                let Call::Composer { request, .. } = call else { unreachable!() };
                                let input = match request {
                                    Request::Sync { input } => *input,
                                    Request::Mutate { mutation } => Input::Mutate { number, mutation },
                                    Request::Read => unreachable!(),
                                };
                                crate::composer::update_document(&node, &resident, input).await
                            }.await;
                            result.unwrap_or_else(|failure| Frame::Refused { number, failure })
                        })));
                        continue;
                    },
                    result = async { match updating.as_mut() { Some(task) => task.await, None => std::future::pending().await } } => {
                        updating = None;
                        result.map(Some).map_err(|error| Failure::unanswered(error.to_string()))
                    }
                };
                let piece = match result {
                    Ok(Some(frame)) => Ok(Watched::Documents { frame: Box::new(frame) }),
                    Ok(None) => return,
                    Err(error) => Err(error),
                };
                let end = piece.is_err();
                if sender.send_wait(piece, std::time::Duration::from_secs(30)).await.is_err() || end {
                    return;
                }
            }
        });
    } else {
        use fastctx_types::access::{Acting, Purpose};
        let request = fastctx_types::watching::Subscription {
            resident,
            composer: Some(name),
            documents: Some(subscription),
            held: None,
            now: false,
            live_only: false,
            goal: false,
            collaboration: None,
        };
        let mut opened = None;
        let mut refused = None;
        for (link, account) in
            node.speakers(&Acting::Here, &machine, Purpose::Agents).map_err(|error| Failure::new(error.to_string()))?
        {
            match link.watch_shared(&machine.name, &account, request.clone()).await {
                Ok(watching) => {
                    opened = Some(watching);
                    break;
                }
                Err(error) if error.not_run => refused = Some(error),
                Err(error) => return Err(error),
            }
        }
        let mut watching =
            opened.ok_or_else(|| refused.unwrap_or_else(|| Failure::not_run("the conversation is unreachable")))?;
        let input = watching
            .input()
            .ok_or_else(|| Failure::unsupported("this machine does not support native composer documents"))?;
        fastctx_platform::task::spawn(async move {
            loop {
                let piece = tokio::select! {
                    () = sender.closed() => return,
                    Some(incoming) = incoming.recv() => { if let Err(error) = input.send(incoming.input) { let _ = sender.send(Err(error)); return; } continue; },
                    piece = watching.next() => piece,
                };
                let piece = match piece {
                    Ok(Some(piece)) => Ok(piece),
                    Ok(None) => return,
                    Err(error) => Err(error),
                };
                let end = piece.is_err();
                if sender.send_wait(piece, std::time::Duration::from_secs(30)).await.is_err() || end {
                    return;
                }
            }
        });
    }
    Ok(receiver)
}
