//! The connections a relay takes from below: a member's session, the relay
//! session of a relay right below, a probe, or one end of a data
//! connection.

use std::net::SocketAddr;
use std::sync::Arc;

use fastctx_net::sign::{PublicKey, Signature};
use fastctx_net::wire::{self, Down, Intent, Letters, PROTOCOL, Refusal, SILENCE, Up};
use fastctx_types::MachineName;
use futures_util::stream::SplitStream;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{Path, Shared, Sub, calls};
use crate::{Out, Socket, refuse, send};

pub(super) async fn connection(
    shared: Arc<Shared>,
    tls: tokio_rustls::TlsAcceptor,
    tcp: TcpStream,
    peer: SocketAddr,
) {
    let opened = tokio::time::timeout(SILENCE, crate::open(&tls, tcp)).await;
    let (mut socket, path, binding, hello) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(_)) | Err(_) => return,
    };
    let Up::Hello { protocol, intent, .. } = hello else {
        refuse(&mut socket, Refusal::Broken { reason: "the first frame must be a hello".to_string() }).await;
        return;
    };
    if protocol != PROTOCOL {
        refuse(&mut socket, Refusal::Protocol { hub: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string() }).await;
        return;
    }
    match (path.as_str(), intent) {
        (wire::CONTROL_PATH, Intent::Session { key, proof, letters }) => {
            member(shared, socket, peer, binding, key, proof, letters).await;
        }
        (wire::CONTROL_PATH, Intent::Relay { key, proof }) => relay(shared, socket, peer, binding, key, proof).await,
        (wire::CONTROL_PATH, Intent::Probe) => {
            if shared.state().up.is_some() {
                let _ = send(&mut socket, &Down::Ready).await;
                let _ = socket.close(None).await;
            } else {
                refuse(&mut socket, shared.not_connected()).await;
            }
        }
        (wire::CONTROL_PATH, Intent::Join { .. } | Intent::Invited { .. }) => {
            let reason = "a machine joins at the hub, not at a relay".to_string();
            refuse(&mut socket, Refusal::NotAdmitted { reason }).await;
        }
        (wire::DATA_PATH, Intent::Data { key, proof, ticket, end, on_behalf }) => {
            super::data::end(shared, socket, binding, key, proof, ticket, end, on_behalf).await;
        }
        _ => refuse(&mut socket, Refusal::Broken { reason: format!("that is not said on {path}") }).await,
    }
}

/// A member's session, carried upward.
async fn member(
    shared: Arc<Shared>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
    letters: Letters,
) {
    let name = match shared.admit(&key, &proof, &binding).await {
        Ok(name) => name,
        Err(reason) => {
            shared.log(format!("{peer}: refused a session: {reason}"));
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let (out, outgoing) = mpsc::unbounded_channel();
    let tag = match shared.attach(&name, Path::Here { out: out.clone() }, letters) {
        Ok(tag) => tag,
        Err(reason) => {
            refuse(&mut socket, reason).await;
            return;
        }
    };
    shared.log(format!("{name} connected from {peer}"));
    let (sink, mut stream) = socket.split();
    let (writer, mut writer_stopped) = write(sink, outgoing);
    let (ended, refusing) = loop {
        let frame = match next(&mut stream, &mut writer_stopped).await {
            Ok(frame) => frame,
            Err(ended) => break (ended, false),
        };
        let handled = match frame {
            Up::Ping { n } => {
                let _ = out.send(Out::Frame(Down::Pong { n }));
                Ok(())
            }
            frame => from_below(&shared, &name, tag, frame),
        };
        if let Err(reason) = handled {
            let _ = out.send(Out::End(reason.clone()));
            break (Some(reason.to_string()), true);
        }
    };
    shared.left(&name, tag);
    drop(out);
    if refusing {
        let _ = writer.await;
    } else {
        writer.abort();
    }
    match ended {
        Some(reason) => shared.log(format!("{name} disconnected: {reason}")),
        None => shared.log(format!("{name} disconnected")),
    }
}

/// The relay session of a relay right below, whose attachments are attached
/// here in turn.
async fn relay(
    shared: Arc<Shared>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    key: PublicKey,
    proof: Signature,
) {
    let relay = match shared.admit(&key, &proof, &binding).await {
        Ok(name) => name,
        Err(reason) => {
            shared.log(format!("{peer}: refused a relay session: {reason}"));
            refuse(&mut socket, reason).await;
            return;
        }
    };
    let generation = shared.next();
    let (out, outgoing) = mpsc::unbounded_channel();
    let opened = {
        let mut state = shared.state();
        if state.up.is_some() {
            let _ = out.send(Out::Frame(Down::Relaying { world: shared.world.clone(), name: relay.clone() }));
            if let Some(earlier) = state.relays.insert(relay.clone(), Sub { generation, out: out.clone() }) {
                let _ = earlier.out.send(Out::End(Refusal::Replaced));
            }
        }
        state.up.is_some()
    };
    if !opened {
        refuse(&mut socket, shared.not_connected()).await;
        return;
    }
    shared.log(format!("{relay} relays through this relay, from {peer}"));
    let (sink, mut stream) = socket.split();
    let (writer, mut writer_stopped) = write(sink, outgoing);
    let (ended, refusing) = loop {
        let frame = match next(&mut stream, &mut writer_stopped).await {
            Ok(frame) => frame,
            Err(ended) => break (ended, false),
        };
        match frame {
            Up::Ping { n } => {
                let _ = out.send(Out::Frame(Down::Pong { n }));
            }
            Up::Attach { name, tag: theirs, letters } => {
                let carried = shared.machine.table().carries(&relay, &name);
                let attached = if carried {
                    let path = Path::Through { relay: relay.clone(), generation, tag: theirs, out: out.clone() };
                    shared.attach(&name, path, letters).map(|_| ())
                } else {
                    Err(Refusal::NotAdmitted { reason: format!("{name}'s route does not lead through {relay}") })
                };
                if let Err(reason) = attached {
                    let refused = Down::To { name, tag: theirs, frame: Box::new(Down::Refused { reason }) };
                    let _ = out.send(Out::Frame(refused));
                }
            }
            Up::Detach { name, tag: theirs } => {
                if let Some(tag) = through(&shared, &name, &relay, generation, theirs) {
                    shared.left(&name, tag);
                }
            }
            Up::For { name, tag: theirs, frame } => {
                let Some(tag) = through(&shared, &name, &relay, generation, theirs) else { continue };
                if let Err(reason) = from_below(&shared, &name, tag, *frame) {
                    shared.log(format!("{name}, through {relay}: {reason}"));
                    shared.end(&name, tag, reason);
                }
            }
            _ => {
                let reason = Refusal::Broken { reason: "that is not said on a relay session".to_string() };
                let _ = out.send(Out::End(reason.clone()));
                break (Some(reason.to_string()), true);
            }
        }
    };
    {
        let mut state = shared.state();
        if state.relays.get(&relay).is_some_and(|sub| sub.generation == generation) {
            state.relays.remove(&relay);
        }
        let attached: Vec<(MachineName, u64)> = state
            .below
            .iter()
            .filter(|(_, below)| {
                matches!(&below.path, Path::Through { relay: by, generation: of, .. } if *by == relay && *of == generation)
            })
            .map(|(name, below)| (name.clone(), below.tag))
            .collect();
        for (name, tag) in attached {
            state.below.remove(&name);
            super::gone(&mut state, &name, tag);
        }
    }
    shared.departed();
    drop(out);
    if refusing {
        let _ = writer.await;
    } else {
        writer.abort();
    }
    match ended {
        Some(reason) => shared.log(format!("{relay} stopped relaying through this relay: {reason}")),
        None => shared.log(format!("{relay} stopped relaying through this relay")),
    }
}

/// This relay's tag for `name`, when it is attached through `relay`'s
/// session `generation` as that relay's attachment `theirs`.
fn through(shared: &Shared, name: &MachineName, relay: &MachineName, generation: u64, theirs: u64) -> Option<u64> {
    let state = shared.state();
    let below = state.below.get(name)?;
    match &below.path {
        Path::Through { relay: by, generation: of, tag, .. } if by == relay && *of == generation && *tag == theirs => {
            Some(below.tag)
        }
        _ => None,
    }
}

/// A frame from `name`, in its attachment `tag`: what the relay does itself,
/// it does; the rest goes up.
fn from_below(shared: &Shared, name: &MachineName, tag: u64, frame: Up) -> Result<(), Refusal> {
    match frame {
        Up::Call { id, message } => calls::call(shared, name, tag, id, message),
        Up::Answer { call, message } => calls::answer(shared, name, tag, call, message),
        Up::Cancel { id } => calls::cancel(shared, name, tag, id),
        frame @ (Up::Send { .. } | Up::Ack { .. } | Up::Ask { .. }) => shared.up_for(name, tag, frame),
        Up::Ping { n } => shared.tell(name, tag, Down::Pong { n }),
        Up::Hello { .. } => {
            return Err(Refusal::Broken { reason: "a second hello on one connection".to_string() });
        }
        Up::Admit { .. } | Up::Attach { .. } | Up::Detach { .. } | Up::For { .. } => {
            return Err(Refusal::Broken { reason: "that is not said on a member's session".to_string() });
        }
    }
    Ok(())
}

type Sink = futures_util::stream::SplitSink<Socket, WsMessage>;

/// Writes what is queued for a connection; a refusal is the last thing
/// written. Returns the task, and what tells when it stopped.
fn write(
    mut sink: Sink,
    mut outgoing: mpsc::UnboundedReceiver<Out>,
) -> (JoinHandle<()>, tokio::sync::oneshot::Receiver<()>) {
    let (stopped, writer_stopped) = tokio::sync::oneshot::channel::<()>();
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
    (writer, writer_stopped)
}

/// The next frame from below; `Err` with why the connection ended, `None`
/// when it simply closed.
async fn next(
    stream: &mut SplitStream<Socket>,
    writer_stopped: &mut tokio::sync::oneshot::Receiver<()>,
) -> Result<Up, Option<String>> {
    loop {
        let next = tokio::select! {
            next = tokio::time::timeout(SILENCE, stream.next()) => next,
            // The writer stopped: the connection was replaced or ended.
            _ = &mut *writer_stopped => return Err(None),
        };
        return match next {
            Err(_) => Err(Some(format!("nothing was heard from it for {} s", SILENCE.as_secs()))),
            Ok(None | Some(Ok(WsMessage::Close(_)))) => Err(None),
            Ok(Some(Err(error))) => Err(Some(fastctx_net::dial::describe(&error))),
            Ok(Some(Ok(WsMessage::Binary(bytes)))) => {
                serde_json::from_slice::<Up>(&bytes).map_err(|error| Some(format!("an unreadable frame: {error}")))
            }
            Ok(Some(Ok(_))) => continue,
        };
    }
}
