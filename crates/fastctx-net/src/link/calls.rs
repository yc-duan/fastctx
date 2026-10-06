//! What this machine says to others: calls on its sessions, and letters.
//!
//! A call is sealed with the session this machine holds with the member it
//! calls, started with a handshake the first time (`super::sessions`). It
//! goes to the hub as a call and comes back as an answer sealed with the
//! same session. A call too large for a control message carries its bytes
//! over a data connection; a call with a data connection opens this
//! machine's end of it, the source, and hands it to the caller's feed once
//! the other end is there. A letter is sealed to its receiver's key, so it
//! can wait at the hub for a receiver that is away.

use std::io;
use std::sync::Arc;

use fastctx_types::MachineName;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use tokio::sync::{mpsc, oneshot};

use super::{CallEvent, CallFailure, Connection, Feed, Inner, Waiter};
use crate::data::Stream;
use crate::envelope::{self, Envelope, Kind};
use crate::message::{Body, LetterBody, LetterId};
use crate::noise::{self, Keys};
use crate::seal::{self, SealPublic};
use crate::sign::{Bytes, Domain, Signed};
use crate::stream_crypto::StreamKey;
use crate::wire::{self, CallStatus, Carried, End, Ticket, Up};

/// How one try of a call ended.
enum Tried {
    Done(Result<RemoteResult, CallFailure>),
    /// The member did not have the session the call came on, so it did not
    /// open the call and nothing of it ran.
    UnknownSession,
}

/// What a call does beside waiting for its answer.
enum Side {
    None,
    /// Sends a large call's bytes, as the source of its data connection.
    Sending(tokio::task::JoinHandle<()>),
    /// Opens the source end of the call's data connection, for the feed.
    Opening(tokio::task::JoinHandle<()>, oneshot::Receiver<Result<Stream, String>>),
}

impl Inner {
    pub(super) async fn call(
        self: &Arc<Self>,
        to: &MachineName,
        call: RemoteCall,
        mut feed: Option<Feed>,
    ) -> Result<RemoteResult, CallFailure> {
        if *to == self.membership.name {
            return Err(CallFailure::Unreachable { name: to.clone(), reason: "it is this machine".to_string() });
        }
        let Some(connection) = self.connected().await else {
            return Err(CallFailure::NotConnected { reason: self.not_connected() });
        };
        let Some(member) = self.member(to).await else {
            return Err(CallFailure::NotMember(to.clone()));
        };
        let Some(seal) = member.seal else {
            return Err(CallFailure::Unreachable {
                name: to.clone(),
                reason: "it has no sealing key yet: it runs a fastctx older than this one".to_string(),
            });
        };
        for again in [false, true] {
            let keys = self.session(&connection, to, &seal).await?;
            match self.try_call(&connection, to, &keys, &call, &mut feed).await {
                Tried::Done(result) => return result,
                Tried::UnknownSession => {
                    self.sessions.forget(to, keys.session).await;
                    if again {
                        return Err(CallFailure::Unreachable {
                            name: to.clone(),
                            reason: "it did not keep the session just made with it; it may have restarted again"
                                .to_string(),
                        });
                    }
                }
            }
        }
        unreachable!("the second try always returns")
    }

    /// The session this machine holds with `to`, started now if it has none.
    async fn session(
        self: &Arc<Self>,
        connection: &Connection,
        to: &MachineName,
        seal: &SealPublic,
    ) -> Result<Arc<Keys>, CallFailure> {
        let slot = self.sessions.slot(to);
        let mut slot = slot.lock().await;
        if let Some(keys) = slot.as_ref() {
            return Ok(keys.clone());
        }
        // Nothing runs on a handshake, so however it fails, nothing ran.
        let unreachable = |reason: String| CallFailure::Unreachable { name: to.clone(), reason };
        let (initiating, first) = noise::initiate(&self.membership.world, &self.membership.name, &self.seal, to, seal)
            .map_err(|error| unreachable(format!("starting a session: {error}")))?;
        let n = self.number(to)?;
        let session = Some(initiating.session);
        let envelope = Envelope {
            world: self.membership.world.clone(),
            from: self.membership.name.clone(),
            to: to.clone(),
            n,
            kind: Kind::Handshake,
            session,
            re: None,
            sealed: Bytes(first),
        };
        let mut no_feed = None;
        let reply = match self.route(connection, to, n, envelope.sign(&self.key), Side::None, &mut no_feed).await {
            Ok(reply) => reply,
            Err(CallFailure::Unknown { reason, .. }) => return Err(unreachable(reason)),
            Err(failure) => return Err(failure),
        };
        if reply.kind == Kind::Refused {
            return Err(unreachable(format!("it refused a session: {}", String::from_utf8_lossy(&reply.sealed.0))));
        }
        if reply.kind != Kind::HandshakeReply || reply.session != session {
            return Err(unreachable(format!("it answered the handshake with {:?}", reply.kind)));
        }
        let keys = Arc::new(
            initiating.finish(&reply.sealed.0).map_err(|error| unreachable(format!("the handshake: {error}")))?,
        );
        *slot = Some(keys.clone());
        Ok(keys)
    }

    async fn try_call(
        self: &Arc<Self>,
        connection: &Connection,
        to: &MachineName,
        keys: &Keys,
        call: &RemoteCall,
        feed: &mut Option<Feed>,
    ) -> Tried {
        let n = match self.number(to) {
            Ok(n) => n,
            Err(failure) => return Tried::Done(Err(failure)),
        };
        let header = envelope::header(
            &self.membership.world,
            &self.membership.name,
            to,
            n,
            Kind::Call,
            &Some(keys.session),
            &None,
        );
        let seal = |body: &Body| {
            let plaintext = serde_json::to_vec(body).expect("bodies serialize");
            Envelope {
                world: self.membership.world.clone(),
                from: self.membership.name.clone(),
                to: to.clone(),
                n,
                kind: Kind::Call,
                session: Some(keys.session),
                re: None,
                sealed: Bytes(keys.seal(n, &header, &plaintext)),
            }
            .sign(&self.key)
        };
        let ticket = || -> Result<(Signed, StreamKey), CallFailure> {
            let unreachable = |error: io::Error| CallFailure::Unreachable {
                name: to.clone(),
                reason: format!("choosing a data connection: {error}"),
            };
            let stream = crate::sign::random::<8>().map(u64::from_le_bytes).map_err(unreachable)?;
            let ticket = Ticket {
                world: self.membership.world.clone(),
                stream,
                source: self.membership.name.clone(),
                sink: to.clone(),
            };
            Ok((Signed::new(&self.key, Domain::Ticket, &ticket), StreamKey::generate().map_err(unreachable)?))
        };
        let (message, side) = if feed.is_some() {
            let (ticket, key) = match ticket() {
                Ok(ticket) => ticket,
                Err(failure) => return Tried::Done(Err(failure)),
            };
            let message = seal(&Body::StreamCall { call: call.clone(), ticket: ticket.clone(), key: key.clone() });
            let (opened, opening) = oneshot::channel();
            let inner = self.clone();
            let task = tokio::spawn(async move {
                let _ = opened.send(Stream::open(&inner.membership, &inner.key, ticket, &key, End::Source).await);
            });
            (message, Side::Opening(task, opening))
        } else {
            let message = seal(&Body::Call { call: call.clone() });
            if wire::encode(&Up::Call { id: u64::MAX, message: message.clone() }).len() <= wire::MAX_CONTROL_FRAME {
                (message, Side::None)
            } else {
                // A call too large for the control connection says what
                // follows over a data connection, which this machine sends
                // as the source.
                let bytes = serde_json::to_vec(call).expect("calls serialize");
                let (ticket, key) = match ticket() {
                    Ok(ticket) => ticket,
                    Err(failure) => return Tried::Done(Err(failure)),
                };
                let carried = Carried {
                    ticket: ticket.clone(),
                    key: key.clone(),
                    bytes: bytes.len() as u64,
                    sha256: crate::sign::Digest(crate::sign::sha256(&bytes)),
                };
                let message = seal(&Body::LargeCall { carried });
                let (inner, name) = (self.clone(), to.clone());
                let task = tokio::spawn(async move {
                    if let Err(error) = crate::data::source(&inner.membership, &inner.key, ticket, &key, &bytes).await {
                        inner.node.log(format!("sending a large call to {name}: {error}"));
                    }
                });
                (message, Side::Sending(task))
            }
        };
        let had_feed = feed.is_some();
        let reply = match self.route(connection, to, n, message, side, feed).await {
            Ok(reply) => reply,
            Err(failure) => return Tried::Done(Err(failure)),
        };
        let broken = |reason: String| Tried::Done(Err(CallFailure::Unknown { name: to.clone(), reason }));
        match reply.kind {
            // Its end of the data connection opens only once it opened the
            // call; a feed already handed its stream cannot be tried again.
            Kind::UnknownSession if had_feed && feed.is_none() => {
                broken("it said it did not have the session after its data connection opened".to_string())
            }
            Kind::UnknownSession => Tried::UnknownSession,
            Kind::Refused => Tried::Done(Ok(Err(fastctx_types::ipc::Failure::new(format!(
                "{to} refused the call: {}",
                String::from_utf8_lossy(&reply.sealed.0)
            ))))),
            Kind::Answer if reply.session == Some(keys.session) => {
                let body = match keys.open(reply.n, &reply.header(), &reply.sealed.0) {
                    Ok(plaintext) => serde_json::from_slice::<Body>(&plaintext),
                    Err(error) => return broken(format!("its answer: {error}")),
                };
                match body {
                    Ok(Body::Answer { result }) => Tried::Done(Ok(result)),
                    Ok(Body::LargeAnswer { carried }) => match self.fetch(to, &carried).await {
                        Ok(result) => Tried::Done(Ok(result)),
                        Err(reason) => {
                            broken(format!("its answer was too large for a message, and fetching it failed: {reason}"))
                        }
                    },
                    Ok(_) => broken("what came back is not an answer".to_string()),
                    Err(error) => broken(format!("its answer cannot be read: {error}")),
                }
            }
            kind => broken(format!("it answered the call with {kind:?}")),
        }
    }

    /// Sends a call-like envelope `message`, numbered `n`, to `to` and waits
    /// for the envelope that answers it, signed by `to`.
    async fn route(
        self: &Arc<Self>,
        connection: &Connection,
        to: &MachineName,
        n: u64,
        message: Signed,
        side: Side,
        feed: &mut Option<Feed>,
    ) -> Result<Envelope, CallFailure> {
        let id = self.next_id();
        let (events, mut received) = mpsc::unbounded_channel();
        self.calls.lock().expect("never poisoned").insert(id, Waiter { events });
        let (sending, mut opening) = match side {
            Side::None => (None, None),
            Side::Sending(task) => (Some(task), None),
            Side::Opening(task, opened) => (Some(task), Some(opened)),
        };
        let mut waiting = Waiting {
            inner: self.clone(),
            id,
            connection: connection.clone(),
            answered: false,
            sending,
            feeding: None,
        };
        if connection.up.send(Up::Call { id, message }).is_err() {
            return Err(CallFailure::NotConnected { reason: "the connection to the hub just ended".to_string() });
        }
        let mut delivered = false;
        let lost = |delivered: bool, reason: String| {
            if delivered {
                CallFailure::Unknown { name: to.clone(), reason }
            } else {
                CallFailure::Unreachable { name: to.clone(), reason }
            }
        };
        let mut fed: Option<oneshot::Receiver<Result<(), String>>> = None;
        loop {
            let event = tokio::select! {
                event = received.recv() => match event {
                    Some(event) => event,
                    None => return Err(lost(delivered, "the link stopped".to_string())),
                },
                opened = async { opening.as_mut().expect("polled only while opening").await }, if opening.is_some() => {
                    opening = None;
                    match opened {
                        Ok(Ok(stream)) => {
                            let feed = feed.take().expect("a call that opens a data connection has a feed");
                            let (done, finished) = oneshot::channel();
                            waiting.feeding = Some(tokio::spawn(async move {
                                let _ = done.send(feed(stream).await);
                            }));
                            fed = Some(finished);
                            continue;
                        }
                        Ok(Err(error)) => return Err(lost(delivered, format!("opening its data connection: {error}"))),
                        Err(_) => return Err(lost(delivered, "opening its data connection stopped".to_string())),
                    }
                }
                finished = async { fed.as_mut().expect("polled only while feeding").await }, if fed.is_some() => {
                    fed = None;
                    match finished {
                        Ok(Err(error)) => return Err(lost(delivered, format!("sending its data failed: {error}"))),
                        // Fed in full; the answer decides.
                        Ok(Ok(())) | Err(_) => continue,
                    }
                }
            };
            match event {
                CallEvent::Status(CallStatus::Delivered) => delivered = true,
                CallEvent::Status(CallStatus::Offline { last_seen }) => {
                    waiting.answered = true;
                    return Err(CallFailure::Offline { name: to.clone(), last_seen });
                }
                CallEvent::Status(CallStatus::NotMember) => {
                    waiting.answered = true;
                    return Err(CallFailure::NotMember(to.clone()));
                }
                CallEvent::Status(CallStatus::Lost) => {
                    waiting.answered = true;
                    return Err(lost(delivered, "its connection to the hub ended".to_string()));
                }
                CallEvent::Answered(signed) => {
                    waiting.answered = true;
                    return self
                        .check_answer(to, n, &signed)
                        .map_err(|reason| CallFailure::Unknown { name: to.clone(), reason });
                }
                CallEvent::Disconnected(reason) => {
                    waiting.answered = true;
                    return Err(lost(delivered, reason));
                }
            }
        }
    }

    /// The envelope answering this machine's call `n` to `to`, if `to`
    /// signed it for this machine.
    fn check_answer(&self, to: &MachineName, n: u64, signed: &Signed) -> Result<Envelope, String> {
        let claimed = envelope::claim(signed)?;
        if claimed.to != self.membership.name || claimed.world != self.membership.world {
            return Err(format!("the answer is for {} in {}", claimed.to, claimed.world));
        }
        if claimed.from != *to {
            return Err(format!("the answer is from {}, not {to}", claimed.from));
        }
        let key = self
            .members()
            .table
            .get(to)
            .map(|member| member.key)
            .ok_or_else(|| format!("{to} is not a member this machine trusts"))?;
        let opened = envelope::open(signed, &key)?;
        if !opened.kind.is_answer() || opened.re != Some(n) {
            return Err(format!("what came back answers call {:?}, not {n}", opened.re));
        }
        Ok(opened)
    }

    /// Takes a large answer `to` sends over a data connection.
    async fn fetch(&self, to: &MachineName, carried: &Carried) -> Result<RemoteResult, String> {
        let ticket: Ticket = carried.ticket.claim().map_err(|error| format!("the ticket: {error}"))?;
        if ticket.source != *to || ticket.sink != self.membership.name {
            return Err(format!("the ticket is for {} to {}", ticket.source, ticket.sink));
        }
        let bytes = crate::data::sink(&self.membership, &self.key, carried).await?;
        serde_json::from_slice(&bytes).map_err(|error| format!("the answer cannot be read: {error}"))
    }

    /// The next number this machine signs with, for something to `to`.
    fn number(&self, to: &MachineName) -> Result<u64, CallFailure> {
        self.counter.next().map_err(|error| CallFailure::Unreachable {
            name: to.clone(),
            reason: format!("numbering the call: {error}"),
        })
    }

    /// Seals a letter to `to` and keeps it in the outbox; it goes to the hub
    /// whenever the link is up.
    pub(super) fn send_letter(&self, to: &MachineName, id: LetterId, letter: Letter) -> io::Result<()> {
        let seal = self.seal_of(to)?;
        let n = self.counter.next()?;
        let message = self.seal_letter(to, &seal, n, id, letter)?;
        let frame = wire::encode(&Up::Send { seq: u64::MAX, message: message.clone() });
        if frame.len() > wire::MAX_CONTROL_FRAME {
            return Err(io::Error::other(format!(
                "a letter of {} bytes is more than a control message holds ({} bytes)",
                frame.len(),
                wire::MAX_CONTROL_FRAME
            )));
        }
        {
            let mut letters = self.letters.lock().expect("never poisoned");
            super::letters::Outbox::new(&self.dir).put(letters.next, &message)?;
            letters.next += 1;
        }
        self.outbox_changed.notify_one();
        Ok(())
    }

    /// The most a letter to `to` takes as a control message, with the
    /// longest id and count it could carry.
    pub(super) fn letter_size(&self, to: &MachineName, letter: &Letter) -> usize {
        // Sealing adds the same number of bytes whoever it is sealed to.
        let seal = self.seal_of(to).unwrap_or_else(|_| self.seal.public());
        match self.seal_letter(to, &seal, u64::MAX, LetterId::longest(), letter.clone()) {
            Ok(message) => wire::encode(&Up::Send { seq: u64::MAX, message }).len(),
            Err(_) => usize::MAX,
        }
    }

    fn seal_of(&self, to: &MachineName) -> io::Result<SealPublic> {
        let members = self.members();
        let member = members
            .table
            .get(to)
            .ok_or_else(|| io::Error::other(format!("{to} is not a member this machine trusts")))?;
        member.seal.ok_or_else(|| {
            io::Error::other(format!("{to} has no sealing key yet: it runs a fastctx older than this one"))
        })
    }

    fn seal_letter(
        &self,
        to: &MachineName,
        seal: &SealPublic,
        n: u64,
        id: LetterId,
        letter: Letter,
    ) -> io::Result<Signed> {
        let world = &self.membership.world;
        let from = &self.membership.name;
        let header = envelope::header(world, from, to, n, Kind::Letter, &None, &None);
        let plaintext = serde_json::to_vec(&LetterBody { id, letter }).map_err(io::Error::other)?;
        let sealed = seal::seal(seal, &letter_info(world, from, to), &header, &plaintext).map_err(io::Error::other)?;
        let envelope = Envelope {
            world: world.clone(),
            from: from.clone(),
            to: to.clone(),
            n,
            kind: Kind::Letter,
            session: None,
            re: None,
            sealed: Bytes(sealed.to_bytes()),
        };
        Ok(envelope.sign(&self.key))
    }
}

/// What a letter's sealing is bound to besides its envelope's header.
pub(super) fn letter_info(world: &crate::trust::WorldId, from: &MachineName, to: &MachineName) -> Vec<u8> {
    let mut info = b"fastctx-world/3/letter\0".to_vec();
    for part in [world.to_string(), from.to_string(), to.to_string()] {
        info.extend_from_slice(part.as_bytes());
        info.push(0);
    }
    info
}

/// Removes a call's waiter when the call is over, tells the hub the caller
/// gave up when it ends without an answer, and stops what the call does
/// beside waiting.
struct Waiting {
    inner: Arc<Inner>,
    id: u64,
    connection: Connection,
    answered: bool,
    sending: Option<tokio::task::JoinHandle<()>>,
    feeding: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        for task in [&self.sending, &self.feeding].into_iter().flatten() {
            task.abort();
        }
        self.inner.calls.lock().expect("never poisoned").remove(&self.id);
        if !self.answered {
            let _ = self.connection.up.send(Up::Cancel { id: self.id });
        }
    }
}
