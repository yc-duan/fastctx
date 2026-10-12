//! The old place after the switch.
//!
//! It forwards every connection to its port to the new hub, byte for byte,
//! so the TLS between a machine and the hub passes through it whole: a
//! machine away during the move comes back to the old address and learns
//! the new one from the new hub. It keeps a connection to the new hub,
//! proving it holds the move's key, to hear how many machines still come
//! through it, and to be told to stop forwarding and, later, to delete the
//! data it kept, disabled. Then it removes its own system service.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fastctx_net::dial;
use fastctx_net::ids::CertPin;
use fastctx_net::moving::HandoverRole;
use fastctx_net::wire::{HEARTBEAT, SILENCE};
use fastctx_platform::{Permissions, publish};
use fastctx_types::hub_move::OldPlace;
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::Instant;

use super::frames::{self, ToNew, ToOld};
use super::{OUTGOING, Outgoing};
use crate::store::{HubFile, Store, json, read};
use crate::{Kept, Leave, Next, log::Log};

pub(crate) async fn run(
    dir: &Path,
    kept: Option<Kept>,
    outgoing: Outgoing,
    log: Arc<Log>,
    leave: &Leave,
) -> anyhow::Result<Next> {
    let why = format!("the hub moved to {} by the move {}", outgoing.to, outgoing.id);
    if matches!(outgoing.old, Some(OldPlace::Deleting { .. } | OldPlace::Deleted { .. })) {
        super::retire(dir, &why, &log, leave);
        return Ok(Next::Again(None));
    }
    let hub: HubFile = read(&Store::hub_file(dir))?;
    let pin = CertPin::of(&std::fs::read(Store::certificate_file(dir))?);
    let (stopper, stopped) = watch::channel(false);
    let forwarder = if matches!(outgoing.old, Some(OldPlace::Forwarding) | None) {
        let kept = crate::bind(kept, hub.listen).await?;
        log.line(format_args!(
            "the hub moved to {} at {}: this place forwards {} to it",
            outgoing.to,
            list(&outgoing.addresses),
            hub.listen
        ));
        Some(tokio::spawn(forward(kept.listener, outgoing.addresses.clone(), log.clone(), stopped)))
    } else {
        log.line(format_args!("the hub moved to {}: this place keeps its old data, disabled", outgoing.to));
        None
    };
    let mut record = outgoing;
    let mut failures = 0;
    // A new hub that stays away is said once, and again when that changes.
    let mut said = String::new();
    loop {
        let mut connected = false;
        tokio::select! {
            talked = talk(dir, &mut record, pin, &log, &stopper, &mut connected) => match talked {
                Ok(()) => {
                    if let Some(forwarder) = forwarder {
                        forwarder.abort();
                    }
                    super::retire(dir, &why, &log, leave);
                    return Ok(Next::Again(None));
                }
                Err(reason) => {
                    if connected || reason != said {
                        log.line(format_args!("the connection to the new hub: {reason}"));
                    }
                    said = reason;
                    failures = if connected { 1 } else { failures + 1 };
                    tokio::select! {
                        () = tokio::time::sleep(fastctx_net::again::backoff(failures)) => {}
                        () = crate::shutdown_signal() => return Ok(Next::Stop),
                    }
                }
            },
            () = crate::shutdown_signal() => {
                log.line(format_args!("shutting down"));
                return Ok(Next::Stop);
            }
        }
    }
}

/// One connection to the new hub, until it ends, or the old data is to be
/// deleted, which is `Ok`.
async fn talk(
    dir: &Path,
    record: &mut Outgoing,
    pin: CertPin,
    log: &Log,
    stopper: &watch::Sender<bool>,
    connected: &mut bool,
) -> Result<(), String> {
    let mut socket = super::out::dial_new(record, pin, HandoverRole::Old).await?;
    *connected = true;
    let forwarding = matches!(record.old, Some(OldPlace::Forwarding) | None);
    frames::send(&mut socket, &ToNew::Old { forwarding, deleted: false }).await?;
    log.line(format_args!("connected to the new hub at {}", list(&record.addresses)));
    let (mut sink, mut stream) = socket.split();
    let mut heard = Instant::now();
    let mut beat = tokio::time::interval(HEARTBEAT);
    loop {
        let message = tokio::select! {
            _ = beat.tick() => {
                frames::send(&mut sink, &ToNew::Working).await?;
                continue;
            }
            () = tokio::time::sleep_until(heard + SILENCE) => {
                return Err(format!("nothing was heard from the new hub for {} s", SILENCE.as_secs()));
            }
            message = stream.next() => message,
        };
        heard = Instant::now();
        let Some(frame) = frames::parse::<ToOld>(message)? else { continue };
        match frame {
            ToOld::Following { machines, devices } => {
                if record.remaining != Some((machines, devices)) {
                    log.line(format_args!("{machines} machines and {devices} devices still come through here"));
                    record.remaining = Some((machines, devices));
                    keep(dir, record, log);
                }
            }
            ToOld::Stop { early } => {
                if matches!(record.old, Some(OldPlace::Forwarding) | None) {
                    log.line(format_args!(
                        "{}; this place stops forwarding and keeps its old data, disabled",
                        if early { "the super administrator stopped this place" } else { "every machine followed" }
                    ));
                    record.old = Some(OldPlace::Stopped { at: Timestamp::now(), early });
                    keep(dir, record, log);
                    stopper.send_replace(true);
                }
            }
            ToOld::Delete => {
                record.old = Some(OldPlace::Deleting { at: Timestamp::now() });
                keep(dir, record, log);
                stopper.send_replace(true);
                // The data goes before the new hub hears it went.
                super::clear(dir, &format!("the hub moved to {}; its old data here was deleted", record.to))
                    .map_err(|error| format!("deleting the old data: {error}"))?;
                frames::send(&mut sink, &ToNew::Deleted).await?;
                let _ = sink.close().await;
                return Ok(());
            }
            ToOld::Refused { reason } => return Err(format!("the new hub refused: {reason}")),
            other => return Err(format!("the new hub said {other:?}")),
        }
    }
}

fn keep(dir: &Path, record: &Outgoing, log: &Log) {
    let written = json(record).and_then(|bytes| publish(&dir.join(OUTGOING), &bytes, Permissions::Private));
    if let Err(error) = written {
        log.line(format_args!("recording how this place stands: {error}"));
    }
}

/// Passes every connection to `listener` to the first of `to` that answers,
/// until it is told to stop; the connections it carries end with it.
async fn forward(listener: TcpListener, to: Vec<SocketAddr>, log: Arc<Log>, mut stopped: watch::Receiver<bool>) {
    let mut carried = tokio::task::JoinSet::new();
    let stop = async move {
        let _ = stopped.wait_for(|stopped| *stopped).await;
    };
    tokio::pin!(stop);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((inbound, peer)) => {
                    dial::configure(&inbound);
                    carried.spawn(pass(inbound, peer, to.clone(), log.clone()));
                }
                // Out of descriptors, for one: the next accept may succeed.
                Err(error) => {
                    log.line(format_args!("accepting: {error}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            },
            Some(_) = carried.join_next(), if !carried.is_empty() => {}
            () = &mut stop => {
                log.line(format_args!("stopped forwarding; {} connections through here end", carried.len()));
                return;
            }
        }
    }
}

async fn pass(mut inbound: TcpStream, peer: SocketAddr, to: Vec<SocketAddr>, log: Arc<Log>) {
    let mut failures = Vec::new();
    for address in &to {
        match TcpStream::connect(address).await {
            Ok(mut outbound) => {
                dial::configure(&outbound);
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                return;
            }
            Err(error) => failures.push(format!("{address}: {error}")),
        }
    }
    log.line(format_args!("{peer}: the new hub could not be reached: {}", failures.join("; ")));
}

fn list(addresses: &[SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
