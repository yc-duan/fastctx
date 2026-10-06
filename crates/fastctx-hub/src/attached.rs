//! Relay sessions: one connection from a relay carrying the sessions of the
//! machines below it (`fastctx_net::tree`).
//!
//! Each machine attached through a relay has a session here like one
//! connected straight: its welcome, its letters, its calls. Only the way out
//! differs: every frame for it goes to the relay, wrapped with its name and
//! attachment, and its frames come in wrapped the same way. The hub takes a
//! relay's word for which machine an attachment is only for a machine whose
//! routes lead through that relay; what such a machine says is signed by it
//! all the same, so a relay can withhold it but not make it up.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{self, Down, Letters, Refusal, SILENCE, Up};
use fastctx_types::MachineName;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{Hub, Out, Socket, control, refuse};

/// A relay's open relay session.
pub(crate) struct RelaySession {
    generation: u64,
    pub(crate) out: mpsc::UnboundedSender<Out>,
}

/// A machine attached through the relay.
struct Attachment {
    tag: u64,
    generation: u64,
    out: mpsc::UnboundedSender<Out>,
    forward: JoinHandle<()>,
}

pub(crate) async fn session(
    hub: Arc<Hub>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
) {
    let relay = match hub.member(&key, &proof, &binding) {
        Ok(name) => name,
        Err(reason) => {
            hub.log.line(format_args!("{peer}: refused a relay session: {reason}"));
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let generation = hub.next();
    let (out, mut outgoing) = mpsc::unbounded_channel();
    let world = hub.store().hub.world.clone();
    let _ = out.send(Out::Frame(Down::Relaying { world, name: relay.clone() }));
    let replaced = hub.relays().insert(relay.clone(), RelaySession { generation, out: out.clone() });
    if let Some(old) = replaced {
        let _ = old.out.send(Out::End(Refusal::Replaced));
    }
    hub.log.line(format_args!("{relay} relays from {peer}"));

    let (mut sink, mut stream) = socket.split();
    let (stopped, mut writer_stopped) = tokio::sync::oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let _stopped = stopped;
        while let Some(out) = outgoing.recv().await {
            let (frame, last) = match out {
                Out::Frame(frame) => (frame, false),
                Out::End(reason) => (Down::Refused { reason }, true),
            };
            if sink.send(WsMessage::Binary(wire::encode(&frame).into())).await.is_err() {
                return;
            }
            if last {
                let _ = sink.close().await;
                return;
            }
        }
    });

    let mut attached: HashMap<MachineName, Attachment> = HashMap::new();
    let (ended, refusing) = loop {
        let next = tokio::select! {
            next = tokio::time::timeout(SILENCE, stream.next()) => next,
            _ = &mut writer_stopped => break (None, false),
        };
        let frame = match next {
            Err(_) => break (Some(format!("nothing was heard from it for {} s", SILENCE.as_secs())), false),
            Ok(None | Some(Ok(WsMessage::Close(_)))) => break (None, false),
            Ok(Some(Err(error))) => break (Some(fastctx_net::dial::describe(&error)), false),
            Ok(Some(Ok(WsMessage::Binary(bytes)))) => match serde_json::from_slice::<Up>(&bytes) {
                Ok(frame) => frame,
                Err(error) => break (Some(format!("an unreadable frame: {error}")), false),
            },
            Ok(Some(Ok(_))) => continue,
        };
        match frame {
            Up::Ping { n } => {
                let _ = out.send(Out::Frame(Down::Pong { n }));
            }
            Up::Attach { name, tag, letters } => attach(&hub, &relay, &out, &mut attached, name, tag, letters),
            Up::Detach { name, tag } => {
                if attached.get(&name).is_some_and(|attachment| attachment.tag == tag) {
                    detach(&hub, &name, attached.remove(&name).expect("just found"));
                }
            }
            Up::For { name, tag, frame } => {
                // A frame of an attachment that ended meanwhile is dropped.
                let Some(attachment) = attached.get(&name).filter(|attachment| attachment.tag == tag) else {
                    continue;
                };
                // The relay speaks for the machine only while this is the
                // machine's session and its route still leads through the
                // relay: one replaced by another connection of it, or left
                // behind by a new route, says nothing more.
                if let Some(reason) = gone_by(&hub, &relay, &name, attachment.generation) {
                    hub.log.line(format_args!("{name}, through {relay}: {reason}"));
                    let attachment = attached.remove(&name).expect("just found");
                    let _ = attachment.out.send(Out::End(Refusal::NotAdmitted { reason }));
                    control::close(&hub, &name, attachment.generation);
                    continue;
                }
                if let Err(reason) = control::handle(&hub, &name, attachment.generation, &attachment.out, *frame) {
                    hub.log.line(format_args!("{name}, through {relay}: {reason}"));
                    let attachment = attached.remove(&name).expect("just found");
                    let _ = attachment.out.send(Out::End(reason));
                    control::close(&hub, &name, attachment.generation);
                }
            }
            _ => {
                let reason = Refusal::Broken { reason: "that is not said on a relay session".to_string() };
                let _ = out.send(Out::End(reason.clone()));
                break (Some(reason.to_string()), true);
            }
        }
    };
    for (name, attachment) in attached {
        detach(&hub, &name, attachment);
    }
    {
        let mut relays = hub.relays();
        if relays.get(&relay).is_some_and(|session| session.generation == generation) {
            relays.remove(&relay);
        }
    }
    drop(out);
    if refusing {
        let _ = writer.await;
    } else {
        writer.abort();
    }
    match ended {
        Some(reason) => hub.log.line(format_args!("{relay} stopped relaying: {reason}")),
        None => hub.log.line(format_args!("{relay} stopped relaying")),
    }
}

/// Opens a session for `name`, attached through `relay`, when its routes
/// lead through it.
fn attach(
    hub: &Hub,
    relay: &MachineName,
    relay_out: &mpsc::UnboundedSender<Out>,
    attached: &mut HashMap<MachineName, Attachment>,
    name: MachineName,
    tag: u64,
    letters: Letters,
) {
    if let Some(earlier) = attached.remove(&name) {
        detach(hub, &name, earlier);
    }
    let carried = {
        let store = hub.store();
        let table = store.table();
        match table.get(&name) {
            None => Err(match table.revoked(&name) {
                Some(revocation) => Refusal::Revoked {
                    by: revocation.by.clone(),
                    at: revocation.at,
                    reason: revocation.reason.clone(),
                },
                None => Refusal::NotMember,
            }),
            Some(_) if !table.carries(relay, &name) => {
                Err(Refusal::NotAdmitted { reason: format!("{name}'s route does not lead through {relay}") })
            }
            Some(_) => Ok(()),
        }
    };
    let generation = hub.next();
    let (out, outgoing) = mpsc::unbounded_channel();
    let forward = tokio::spawn(forward(outgoing, relay_out.clone(), name.clone(), tag));
    let opened = carried.and_then(|()| control::open(hub, &name, generation, out.clone(), letters));
    match opened {
        Ok(()) => {
            hub.log.line(format_args!("{name} connected through {relay}"));
            attached.insert(name, Attachment { tag, generation, out, forward });
        }
        Err(reason) => {
            hub.log.line(format_args!("{relay}: refused to attach {name}: {reason}"));
            let _ = out.send(Out::End(reason));
        }
    }
}

/// Why the attachment of `name` in `generation`, through `relay`, no longer
/// speaks for it, if it does not.
fn gone_by(hub: &Hub, relay: &MachineName, name: &MachineName, generation: u64) -> Option<String> {
    let store = hub.store();
    if !store.table().carries(relay, name) {
        return Some(format!("{name}'s route no longer leads through {relay}"));
    }
    let current = hub.sessions().get(name).is_some_and(|session| session.generation == generation);
    (!current).then(|| format!("another connection of {name} replaced this one"))
}

fn detach(hub: &Hub, name: &MachineName, attachment: Attachment) {
    control::close(hub, name, attachment.generation);
    // What was queued for it before it left is of no use to the relay.
    attachment.forward.abort();
    hub.log.line(format_args!("{name} disconnected"));
}

/// Wraps every frame for an attached machine for its relay, through the one
/// that ends its session.
async fn forward(
    mut outgoing: mpsc::UnboundedReceiver<Out>,
    relay: mpsc::UnboundedSender<Out>,
    name: MachineName,
    tag: u64,
) {
    while let Some(out) = outgoing.recv().await {
        let (frame, last) = match out {
            Out::Frame(frame) => (frame, false),
            Out::End(reason) => (Down::Refused { reason }, true),
        };
        let wrapped = Down::To { name: name.clone(), tag, frame: Box::new(frame) };
        if relay.send(Out::Frame(wrapped)).is_err() || last {
            return;
        }
    }
}
