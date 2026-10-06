//! The relay session upward: through the first of the relay's upstreams that
//! takes it, carrying the sessions of the machines below; then, when it
//! ends, connecting again. A relay on a fallback asks its preferred
//! upstreams now and then whether they take sessions again, as a machine's
//! link does, and returns to the first that does.

use std::sync::Arc;
use std::time::Duration;

use fastctx_net::dial::{self, HubSocket};
use fastctx_net::tree::{Endpoint, Upstream};
use fastctx_net::wire::{self, Down, HEARTBEAT, Intent, PROTOCOL, Refusal, SILENCE, Up};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{Shared, Upward, calls};
use crate::Out;

/// How often a relay on a fallback upstream asks its preferred ones whether
/// they take sessions again; as for a machine's link.
const RETURN: Duration = Duration::from_secs(60);

/// A relay session that stays up this long counts as healthy: the next
/// failure is tried again at once.
const STABLE: Duration = Duration::from_secs(60);

enum Ended {
    Retry(String),
    Reroute(String),
    /// This machine was revoked.
    Stop(String),
}

pub(super) async fn run(shared: Arc<Shared>) {
    let mut failures: u32 = 0;
    let mut stopping = shared.stopping.subscribe();
    loop {
        if *stopping.borrow_and_update() {
            return;
        }
        let rerouted = shared.rerouted.notified();
        tokio::pin!(rerouted);
        rerouted.as_mut().enable();
        let endpoints = shared.machine.upstreams();
        let mut tried = Vec::new();
        let mut ended = None;
        for (index, endpoint) in endpoints.iter().enumerate() {
            match open(&shared, endpoint).await {
                Ok(socket) => {
                    let since = Instant::now();
                    ended = Some((carry(&shared, socket, &endpoints[..=index]).await, since.elapsed()));
                    break;
                }
                Err(Ended::Retry(reason)) => tried.push(format!("{}: {reason}", endpoint.upstream)),
                Err(other) => {
                    ended = Some((other, Duration::ZERO));
                    break;
                }
            }
        }
        match ended {
            Some((Ended::Stop(reason), _)) => {
                shared.log(format!("the relay stopped: {reason}"));
                return;
            }
            Some((Ended::Reroute(reason), _)) => {
                shared.log(format!("the relay connects again: {reason}"));
                failures = 0;
                continue;
            }
            Some((Ended::Retry(reason), lasted)) => {
                if lasted >= STABLE {
                    failures = 0;
                }
                shared.log(format!("the relay lost its upstream: {reason}"));
            }
            None => {
                if failures == 0 {
                    shared.log(format!("the relay reaches no upstream: {}", tried.join("; ")));
                }
            }
        }
        failures = failures.saturating_add(1);
        tokio::select! {
            () = tokio::time::sleep(dial::backoff(failures)) => {}
            () = &mut rerouted => failures = 0,
            _ = stopping.changed() => return,
        }
    }
}

async fn open(shared: &Shared, endpoint: &Endpoint) -> Result<HubSocket, Ended> {
    let dialed = dial::dial(&endpoint.addresses, endpoint.cert, wire::CONTROL_PATH, wire::MAX_RELAYED_FRAME)
        .await
        .map_err(Ended::Retry)?;
    let mut socket = dialed.socket;
    let hello = Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Relay { key: shared.key, proof: shared.machine.prove(&dialed.binding) },
    };
    match tokio::time::timeout(SILENCE, dial::send(&mut socket, &hello)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(Ended::Retry(error)),
        Err(_) => return Err(Ended::Retry("sending the hello timed out".to_string())),
    }
    match tokio::time::timeout(SILENCE, dial::recv(&mut socket)).await {
        Ok(Ok(Some(Down::Relaying { world, name }))) if world == shared.world && name == shared.me => Ok(socket),
        Ok(Ok(Some(Down::Relaying { world, name }))) => {
            Err(Ended::Retry(format!("it knows this machine as {name} in {world}")))
        }
        Ok(Ok(Some(Down::Refused { reason: reason @ Refusal::Revoked { .. } }))) => Err(Ended::Stop(reason.to_string())),
        Ok(Ok(Some(Down::Refused { reason }))) => Err(Ended::Retry(reason.to_string())),
        Ok(Ok(Some(_))) => Err(Ended::Retry("the first frame did not open a relay session".to_string())),
        Ok(Ok(None)) => Err(Ended::Retry("the connection was closed".to_string())),
        Ok(Err(error)) => Err(Ended::Retry(error)),
        Err(_) => Err(Ended::Retry(format!("no answer within {} s", SILENCE.as_secs()))),
    }
}

/// Carries the sessions below over `socket` until it ends.
async fn carry(shared: &Arc<Shared>, socket: HubSocket, tried: &[Endpoint]) -> Ended {
    let endpoint = tried.last().expect("a relay session is opened through an upstream").clone();
    let rerouted = shared.rerouted.notified();
    tokio::pin!(rerouted);
    rerouted.as_mut().enable();

    let (out, mut outgoing) = mpsc::unbounded_channel::<Up>();
    let generation = shared.next();
    shared.state().up = Some(Upward { generation, out: out.clone(), endpoint: endpoint.clone(), tried: tried.to_vec() });
    shared.machine.ready(true);
    shared.log(format!("the relay carries the machines below it through {}", endpoint.upstream));

    let (mut sink, mut stream) = socket.split();
    let (writer_done, mut writer_stopped) = oneshot::channel::<()>();
    let mut tasks = vec![tokio::spawn(async move {
        let _done = writer_done;
        while let Some(frame) = outgoing.recv().await {
            if sink.send(WsMessage::Binary(wire::encode(&frame).into())).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    })];
    {
        let out = out.clone();
        tasks.push(tokio::spawn(async move {
            let mut n = 0;
            loop {
                tokio::time::sleep(HEARTBEAT).await;
                n += 1;
                if out.send(Up::Ping { n }).is_err() {
                    return;
                }
            }
        }));
    }
    let (returned, mut preferred_back) = oneshot::channel::<Upstream>();
    if tried.len() > 1 {
        let preferred = tried[..tried.len() - 1].to_vec();
        tasks.push(tokio::spawn(async move {
            loop {
                tokio::time::sleep(RETURN).await;
                for endpoint in &preferred {
                    if dial::probe(&endpoint.addresses, endpoint.cert).await.is_ok() {
                        let _ = returned.send(endpoint.upstream.clone());
                        return;
                    }
                }
            }
        }));
    }

    let mut stopping = shared.stopping.subscribe();
    // A stop that came while the session was being opened is seen at once.
    stopping.mark_changed();
    let mut last_pong = Instant::now();
    let mut last_ping = 0;
    // The root revoked this machine: its own session below is told so.
    let mut revoked = None;
    let ended = loop {
        let next = tokio::select! {
            biased;
            _ = stopping.changed() => {
                if *stopping.borrow() {
                    break Ended::Stop(format!("{} stopped relaying", shared.me));
                }
                continue;
            }
            _ = tokio::time::sleep_until(last_pong + SILENCE) => {
                break Ended::Retry(format!("the upstream did not answer a heartbeat for {} s", SILENCE.as_secs()));
            }
            _ = &mut writer_stopped => break Ended::Retry("writing to the upstream failed".to_string()),
            () = &mut rerouted => break Ended::Reroute("the upstreams this relay connects through changed".to_string()),
            Ok(upstream) = &mut preferred_back => break Ended::Reroute(format!("{upstream} takes sessions again")),
            next = stream.next() => next,
        };
        let frame: Down = match next {
            None | Some(Ok(WsMessage::Close(_))) => break Ended::Retry("the upstream closed the connection".to_string()),
            Some(Err(error)) => break Ended::Retry(dial::describe(&error)),
            Some(Ok(WsMessage::Binary(bytes))) => match serde_json::from_slice(&bytes) {
                Ok(frame) => frame,
                Err(error) => break Ended::Retry(format!("an unreadable frame from the upstream: {error}")),
            },
            Some(Ok(_)) => continue,
        };
        match frame {
            Down::Pong { n } => {
                if n > last_ping {
                    last_ping = n;
                    last_pong = Instant::now();
                }
            }
            Down::To { name, tag, frame } => down(shared, name, tag, *frame),
            Down::Refused { reason: reason @ Refusal::Revoked { .. } } => {
                revoked = Some(reason.clone());
                break Ended::Stop(reason.to_string());
            }
            Down::Refused { reason } => break Ended::Retry(reason.to_string()),
            _ => break Ended::Retry("the upstream sent a frame that is not said on a relay session".to_string()),
        }
    };
    if *stopping.borrow() {
        // What the machines below said before the relay stopped is written
        // before the session closes: the writer ends once every sender of
        // frames is gone.
        for task in &tasks[1..] {
            task.abort();
        }
        drop(out);
        let _ = tasks.swap_remove(0).await;
        return ended;
    }
    for task in tasks {
        task.abort();
    }
    let reason = match &ended {
        Ended::Retry(reason) | Ended::Reroute(reason) | Ended::Stop(reason) => reason.clone(),
    };
    let ours = shared.state().up.as_ref().is_some_and(|up| up.generation == generation);
    if ours {
        shared.machine.ready(false);
        let lost = Refusal::Broken { reason: format!("the relay {} lost its upstream: {reason}", shared.me) };
        match &revoked {
            Some(revocation) => shared.lost_to(revocation.clone(), lost),
            None => shared.lost(lost),
        }
    }
    ended
}

/// A frame from above for `name`'s attachment `tag`.
fn down(shared: &Shared, name: fastctx_types::MachineName, tag: u64, frame: Down) {
    let mut state = shared.state();
    let Some(below) = state.below.get(&name).filter(|below| below.tag == tag) else {
        // It left meanwhile; the upstream hears so.
        return;
    };
    match frame {
        Down::Welcome { .. } => {
            below.send(&name, Out::Frame(frame));
            if let Some(below) = state.below.get_mut(&name) {
                below.welcomed = true;
            }
        }
        Down::Incoming { call, message } => calls::incoming(shared, &mut state, &name, tag, call, message),
        Down::Refused { reason } => {
            below.send(&name, Out::End(reason));
            state.below.remove(&name);
            super::gone(&mut state, &name, tag);
            drop(state);
            shared.departed();
        }
        frame => {
            below.send(&name, Out::Frame(frame));
        }
    }
}
