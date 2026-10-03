//! Authenticated, ticket-bound stream pairing with per-connection backpressure.

use super::session::{Link, Peer};
use super::{Hub, log};
use crate::world::wire::{Frame, StreamRole};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// The end that arrived second, as handed to the end that waited.
struct Partner {
    name: String,
    to_partner: mpsc::Sender<Message>,
}

struct Waiting {
    ticket: crate::world::stream_ticket::Ticket,
    role: StreamRole,
    name: String,
    to_waiting: mpsc::Sender<Message>,
    partner: oneshot::Sender<Partner>,
}

enum Pairing {
    Paired(Partner),
    Wait(oneshot::Receiver<Partner>),
    /// The same member or the same role tried to take the other end.
    Refused,
}

/// Streams waiting for their other end, and how many connections each member holds.
#[derive(Default)]
pub(crate) struct Registry {
    waiting: Mutex<HashMap<u64, Waiting>>,
    active: Mutex<HashMap<String, usize>>,
    total: AtomicUsize,
}

impl Registry {
    fn admit(&self, name: &str) {
        let mut active = self.active.lock();
        let count = active.entry(name.to_string()).or_insert(0);
        *count += 1;
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    fn release(&self, name: &str) {
        let mut active = self.active.lock();
        if let Some(count) = active.get_mut(name) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                active.remove(name);
            }
        }
        self.total.fetch_sub(1, Ordering::Relaxed);
    }

    fn pair(
        &self,
        stream: u64,
        role: StreamRole,
        name: &str,
        to_me: mpsc::Sender<Message>,
        ticket: &crate::world::stream_ticket::Ticket,
    ) -> Pairing {
        let mut waiting = self.waiting.lock();
        match waiting.remove(&stream) {
            Some(other) if other.role != role && other.name != name && &other.ticket == ticket => {
                let _ = other.partner.send(Partner {
                    name: name.to_string(),
                    to_partner: to_me,
                });
                Pairing::Paired(Partner {
                    name: other.name,
                    to_partner: other.to_waiting,
                })
            }
            Some(other) => {
                waiting.insert(stream, other);
                Pairing::Refused
            }
            None => {
                let (partner, receiver) = oneshot::channel();
                waiting.insert(
                    stream,
                    Waiting {
                        ticket: ticket.clone(),
                        role,
                        name: name.to_string(),
                        to_waiting: to_me,
                        partner,
                    },
                );
                Pairing::Wait(receiver)
            }
        }
    }

    fn forget(&self, stream: u64) {
        self.waiting.lock().remove(&stream);
    }

    /// Active data connections, for the status file.
    pub(crate) fn active_count(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
}

struct Admitted<'a> {
    registry: &'a Registry,
    name: String,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        self.registry.release(&self.name);
    }
}

/// Runs one end of a data connection from a completed handshake to close.
pub(crate) async fn attach<S>(
    hub: Arc<Hub>,
    mut link: Link<S>,
    peer: Peer,
    stream: u64,
    role: StreamRole,
    ticket: Option<crate::world::stream_ticket::Ticket>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let registry = &hub.streams;
    let ticket = match validate_ticket(&hub, &peer.name, stream, role, ticket) {
        Ok(ticket) => ticket,
        Err(error) => {
            refuse(&mut link, &error).await;
            return;
        }
    };
    registry.admit(&peer.name);
    let _admitted = Admitted {
        registry,
        name: peer.name.clone(),
    };
    let (to_me, from_partner) = mpsc::channel::<Message>(1);
    let partner = match registry.pair(stream, role, &peer.name, to_me, &ticket) {
        Pairing::Paired(partner) => partner,
        Pairing::Refused => {
            refuse(
                &mut link,
                "the other end of this stream is the same member or the same role",
            )
            .await;
            return;
        }
        Pairing::Wait(receiver) => {
            tokio::select! {
                partner = receiver => match partner { Ok(partner) => partner, Err(_) => { registry.forget(stream); return; } },
                message = link.socket.next() => { registry.forget(stream); if message.is_some() { refuse(&mut link, "stream closed or sent data before pairing").await; } return; },
                () = hub.shutdown.cancelled() => { registry.forget(stream); return; }
            }
        }
    };
    if let Err(error) = link.send(&Frame::Attached { stream }).await {
        log(format!(
            "\"{}\": cannot confirm data connection {stream}: {error}",
            peer.name
        ));
        return;
    }
    log(format!(
        "data connection {stream}: \"{}\" ({role:?}) paired with \"{}\"",
        peer.name, partner.name
    ));
    pump(link, from_partner, partner.to_partner).await;
    log(format!(
        "data connection {stream}: \"{}\" ({role:?}) closed",
        peer.name
    ));
}

async fn refuse<S>(link: &mut Link<S>, reason: &str)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _ = link
        .send(&Frame::Bye {
            reason: reason.to_string(),
        })
        .await;
    link.close().await;
}

/// Forwards this end's messages to the partner and the partner's to this end until either
/// side closes. Dropping `to_partner` is how the partner learns this end is gone.
async fn pump<S>(
    link: Link<S>,
    mut from_partner: mpsc::Receiver<Message>,
    to_partner: mpsc::Sender<Message>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut source) = link.socket.split();
    let forward = async {
        while let Some(item) = source.next().await {
            match item {
                Ok(message @ Message::Binary(_)) => {
                    if to_partner.send(message).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Text(_) | Message::Close(_)) | Err(_) => break,
            }
        }
    };
    let backward = async {
        while let Some(message) = from_partner.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    };
    tokio::select! {
        () = forward => {}
        () = backward => {}
    }
}

fn validate_ticket(
    hub: &Hub,
    name: &str,
    stream: u64,
    role: StreamRole,
    ticket: Option<crate::world::stream_ticket::Ticket>,
) -> Result<crate::world::stream_ticket::Ticket, String> {
    let ticket = ticket.ok_or("A data stream requires a signed ticket")?;
    if ticket.world != hub.world_id
        || ticket.stream != stream
        || ticket.endpoint(role) != name
        || ticket.source == ticket.sink
    {
        return Err("stream ticket identity mismatch".into());
    }
    let source = hub
        .store
        .member(&ticket.source)?
        .ok_or("unknown stream source")?;
    let sink = hub
        .store
        .member(&ticket.sink)?
        .ok_or("unknown stream sink")?;
    if source.is_revoked() || sink.is_revoked() {
        return Err("revoked stream endpoint".into());
    }
    ticket.verify(&crate::world::crypto::b64_array(
        &source.node_pub,
        "stream author",
    )?)?;
    if !((ticket.principal == ticket.source
        && ticket.target == ticket.sink
        && ticket.verb == "copy")
        || (ticket.principal == ticket.sink && ticket.target == ticket.source))
    {
        return Err("stream grant does not name its endpoints".into());
    }
    hub.check_grant(&ticket.principal, Some(&ticket.verb), &ticket.target)?;
    Ok(ticket)
}
