//! What this machine says to others: calls, and letters.
//!
//! A call goes in an envelope naming the account it is for and what it asks
//! of the receiver, and comes back as an answer: over the direct link with
//! the receiver when there is one, else through the hub. A call too large
//! for a control message carries its bytes over a data connection; a call
//! with a data connection opens this machine's end of it, the source, and
//! hands it to the caller's feed once the other end is there. A letter
//! waits in the outbox until the hub holds it: letters always go through
//! the hub, which keeps them for machines that are away.

use std::io;
use std::sync::Arc;

use fastctx_types::access::Purpose;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use fastctx_types::{AccountName, MachineName};
use tokio::sync::{mpsc, oneshot};

/// How long the answer to a call is waited for once its data connection
/// broke: a machine that refuses the call ends the connection before it
/// answers.
const ANSWER_AFTER_BREAK: std::time::Duration = std::time::Duration::from_secs(5);

use super::direct::{DataWay, PeerConn, Worked};
use super::{CallEvent, CallFailure, CallOn, Connection, Feed, Inner, Waiter};
use crate::data::{Endpoint, Stream};
use crate::envelope::{Envelope, Kind};
use crate::message::{Body, LetterBody, LetterId};
use crate::wire::{self, CallStatus, Carried, End, Ticket, Up};

/// What a call goes out on.
#[derive(Clone)]
pub(super) enum Via {
    Hub(Connection),
    Peer(Arc<PeerConn>),
}

impl Via {
    fn on(&self) -> CallOn {
        match self {
            Via::Hub(connection) => CallOn::Hub(connection.generation),
            Via::Peer(conn) => conn.on(),
        }
    }

    fn send_call(&self, id: u64, message: Envelope) -> Result<(), ()> {
        match self {
            Via::Hub(connection) => connection.up.send(Up::Call { id, message }),
            Via::Peer(conn) => conn.send(crate::peer::Frame::Call { id, message }),
        }
    }

    /// Tells the hub the caller gave up the call `id`. Over a direct link
    /// an answer nobody waits for is dropped where it arrives.
    fn cancel(&self, id: u64) {
        if let Via::Hub(connection) = self {
            let _ = connection.up.send(Up::Cancel { id });
        }
    }

    /// Where the data connections of a call made over it go.
    fn endpoint(&self, inner: &Arc<Inner>) -> Endpoint {
        match self {
            Via::Hub(_) => inner.data_endpoint(),
            Via::Peer(conn) => Endpoint::Peer(DataWay::new(inner.clone(), conn.clone(), None)),
        }
    }
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
        account: &AccountName,
        purpose: Purpose,
        call: RemoteCall,
        mut feed: Option<Feed>,
    ) -> Result<RemoteResult, CallFailure> {
        // What a name the machine had before it was renamed names is called
        // by its name now.
        let view = self.view();
        if view.is_me(to) {
            return Err(CallFailure::Unreachable { name: to.clone(), reason: "it is this machine".to_string() });
        }
        let to = &view.canonical(to).clone();
        drop(view);
        // A phone links directly with a machine only while it has calls to
        // it; the first goes through the hub while the link comes up.
        let _demand = self.demand(to);
        let via = match self.direct_link_for(to, account, &call) {
            Some(conn) => {
                if let Some(resident) = crate::message::worked_for(&self.me(), &call) {
                    self.direct.note_worked(Worked { to: to.clone(), account: account.clone(), purpose, resident });
                    if self.connection().is_some() && self.view().direct_links {
                        let inner = self.clone();
                        tokio::spawn(async move { super::session::tell_worked(&inner).await });
                    }
                }
                Via::Peer(conn)
            }
            None => match self.connected().await {
                Some(connection) => Via::Hub(connection),
                None => return Err(CallFailure::NotConnected { reason: self.not_connected() }),
            },
        };
        let n = self.number(to)?;
        let ticket = || -> Result<Ticket, CallFailure> {
            let stream = crate::sign::random::<8>().map(u64::from_le_bytes).map_err(|error| {
                CallFailure::Unreachable { name: to.clone(), reason: format!("choosing a data connection: {error}") }
            })?;
            Ok(Ticket { stream, source: self.me(), sink: to.clone() })
        };
        let envelope = |body: &Body, stream: Option<Ticket>| Envelope {
            hub: self.hub.clone(),
            from: self.me(),
            to: to.clone(),
            n,
            kind: Kind::Call,
            account: Some(account.clone()),
            purpose: Some(purpose),
            re: None,
            stream,
            body: serde_json::to_value(body).expect("bodies serialize"),
        };
        let (message, side) = if feed.is_some() {
            let ticket = ticket()?;
            let message = envelope(&Body::StreamCall { call }, Some(ticket.clone()));
            let (opened, opening) = oneshot::channel();
            let (inner, endpoint) = (self.clone(), via.endpoint(self));
            let task = tokio::spawn(async move {
                let _ = opened.send(Stream::open(&endpoint, &inner.key, ticket.stream, End::Source).await);
            });
            (message, Side::Opening(task, opening))
        } else {
            let message = envelope(&Body::Call { call: call.clone() }, None);
            if wire::encode(&Up::Call { id: u64::MAX, message: message.clone() }).len() <= wire::MAX_CONTROL_FRAME {
                (message, Side::None)
            } else {
                // A call too large for the control connection says what
                // follows over a data connection, which this machine sends
                // as the source.
                let bytes = serde_json::to_vec(&call).expect("calls serialize");
                let ticket = ticket()?;
                let carried = Carried {
                    stream: ticket.stream,
                    bytes: bytes.len() as u64,
                    sha256: crate::sign::Digest(crate::sign::sha256(&bytes)),
                };
                let message = envelope(&Body::LargeCall { carried }, Some(ticket.clone()));
                let (inner, name, endpoint) = (self.clone(), to.clone(), via.endpoint(self));
                let task = tokio::spawn(async move {
                    if let Err(error) = crate::data::source(&endpoint, &inner.key, ticket.stream, &bytes).await {
                        inner.node.log(format!("sending a large call to {name}: {error}"));
                    }
                });
                (message, Side::Sending(task))
            }
        };
        let reply = self.route(&via, to, n, message, side, &mut feed).await?;
        let broken = |reason: String| CallFailure::Unknown { name: to.clone(), reason };
        match reply.kind {
            Kind::Refused => {
                let reason: String = reply.body().unwrap_or_else(|_| "it gave no reason".to_string());
                Ok(Err(fastctx_types::ipc::Failure::new(format!("{to} refused the call: {reason}"))))
            }
            Kind::Answer => match reply.body::<Body>() {
                Ok(Body::Answer { result }) => Ok(result),
                Ok(Body::LargeAnswer { carried }) => self.fetch(&via, to, &reply, &carried).await.map_err(|reason| {
                    broken(format!("its answer was too large for a message, and fetching it failed: {reason}"))
                }),
                Ok(_) => Err(broken("what came back is not an answer".to_string())),
                Err(error) => Err(broken(format!("its answer {error}"))),
            },
            kind => Err(broken(format!("it answered the call with {kind:?}"))),
        }
    }

    /// Sends a call `message`, numbered `n`, to `to` and waits for the
    /// envelope that answers it.
    async fn route(
        self: &Arc<Self>,
        via: &Via,
        to: &MachineName,
        n: u64,
        message: Envelope,
        side: Side,
        feed: &mut Option<Feed>,
    ) -> Result<Envelope, CallFailure> {
        let id = self.next_id();
        let (events, mut received) = mpsc::unbounded_channel();
        self.calls.lock().expect("never poisoned").insert(id, Waiter { events, on: via.on() });
        let (sending, mut opening) = match side {
            Side::None => (None, None),
            Side::Sending(task) => (Some(task), None),
            Side::Opening(task, opened) => (Some(task), Some(opened)),
        };
        let mut waiting =
            Waiting { inner: self.clone(), id, via: via.clone(), answered: false, sending, feeding: None };
        let result = async {
            if via.send_call(id, message).is_err() {
                return Err(match via {
                    Via::Hub(_) => {
                        CallFailure::NotConnected { reason: "the connection to the hub just ended".to_string() }
                    }
                    Via::Peer(_) => CallFailure::Unreachable {
                        name: to.clone(),
                        reason: "the direct link with it just ended".to_string(),
                    },
                });
            }
            // Once queued to the connection, the call may have reached its
            // target even if the hub's Delivered status never reached us.
            let lost = |reason: String| CallFailure::Unknown { name: to.clone(), reason };
            let mut fed: Option<oneshot::Receiver<Result<(), String>>> = None;
            // Its data connection broke: the answer, which may say why, is
            // given a moment to come by its own way.
            let mut broke: Option<(String, std::pin::Pin<Box<tokio::time::Sleep>>)> = None;
            loop {
                let event = tokio::select! {
                    event = received.recv() => match event {
                        Some(event) => event,
                        None => return Err(lost("the link stopped".to_string())),
                    },
                    () = async { broke.as_mut().expect("polled only once broken").1.as_mut().await }, if broke.is_some() => {
                        let (error, _) = broke.take().expect("broken");
                        return Err(lost(format!("sending its data failed: {error}")));
                    }
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
                            Ok(Err(error)) => return Err(lost(format!("opening its data connection: {error}"))),
                            Err(_) => return Err(lost("opening its data connection stopped".to_string())),
                        }
                    }
                    finished = async { fed.as_mut().expect("polled only while feeding").await }, if fed.is_some() => {
                        fed = None;
                        match finished {
                            Ok(Err(error)) => {
                                broke = Some((error, Box::pin(tokio::time::sleep(ANSWER_AFTER_BREAK))));
                                continue;
                            }
                            // Fed in full; the answer decides.
                            Ok(Ok(())) | Err(_) => continue,
                        }
                    }
                };
                match event {
                    CallEvent::Status(CallStatus::Delivered) => {}
                    CallEvent::Status(CallStatus::Offline { last_seen }) => {
                        waiting.answered = true;
                        return Err(CallFailure::Offline { name: to.clone(), last_seen });
                    }
                    CallEvent::Status(CallStatus::NotMember) => {
                        waiting.answered = true;
                        return Err(self.not_member(to));
                    }
                    CallEvent::Status(CallStatus::Refused { reason }) => {
                        waiting.answered = true;
                        return Err(CallFailure::Refused { name: to.clone(), reason });
                    }
                    CallEvent::Status(CallStatus::Lost) => {
                        waiting.answered = true;
                        return Err(lost("its connection to the hub ended".to_string()));
                    }
                    CallEvent::Status(CallStatus::Cut { reason }) => {
                        waiting.answered = true;
                        return Err(lost(reason));
                    }
                    CallEvent::Answered(envelope) => {
                        waiting.answered = true;
                        return self
                            .check_answer(to, n, envelope)
                            .map_err(|reason| CallFailure::Unknown { name: to.clone(), reason });
                    }
                    CallEvent::Disconnected(reason) => {
                        waiting.answered = true;
                        return Err(lost(reason));
                    }
                }
            }
        }
        .await;
        // abort only requests cancellation. A running feed can still commit
        // bytes and resume offsets before its next suspension, so its caller
        // must not start a replacement feed until this one has been dropped.
        if let Some(task) = waiting.feeding.take() {
            task.abort();
            let _ = task.await;
        }
        result
    }

    /// The envelope answering this machine's call `n` to `to`, if it is one.
    fn check_answer(&self, to: &MachineName, n: u64, envelope: Envelope) -> Result<Envelope, String> {
        let view = self.view();
        if !view.is_me(&envelope.to) || envelope.hub != self.hub {
            return Err(format!("the answer is for {} on {}", envelope.to, envelope.hub));
        }
        if !view.same(&envelope.from, to) {
            return Err(format!("the answer is from {}, not {to}", envelope.from));
        }
        if !envelope.kind.is_answer() || envelope.re != Some(n) {
            return Err(format!("what came back answers call {:?}, not {n}", envelope.re));
        }
        Ok(envelope)
    }

    /// Takes a large answer `to` sends over the data connection its answer
    /// names.
    async fn fetch(
        self: &Arc<Self>,
        via: &Via,
        to: &MachineName,
        answer: &Envelope,
        carried: &Carried,
    ) -> Result<RemoteResult, String> {
        let ticket = answer.stream.as_ref().ok_or("the answer names no data connection")?;
        let view = self.view();
        if !view.same(&ticket.source, to) || !view.is_me(&ticket.sink) || ticket.stream != carried.stream {
            return Err(format!("its data connection is for {} to {}", ticket.source, ticket.sink));
        }
        let bytes = crate::data::sink(&via.endpoint(self), &self.key, carried).await?;
        serde_json::from_slice(&bytes).map_err(|error| format!("the answer cannot be read: {error}"))
    }

    /// The next number this machine sends under, for something to `to`.
    fn number(&self, to: &MachineName) -> Result<u64, CallFailure> {
        self.counter.next().map_err(|error| CallFailure::Unreachable {
            name: to.clone(),
            reason: format!("numbering the call: {error}"),
        })
    }

    /// Keeps a letter to `to` in the outbox; it goes to the hub whenever
    /// the link is up.
    pub(super) fn send_letter(
        &self,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        id: LetterId,
        letter: Letter,
    ) -> io::Result<()> {
        let n = self.counter.next()?;
        let message = self.letter(to, account, purpose, n, id, letter);
        let frame = wire::encode(&Up::Send { seq: u64::MAX, message: message.clone() });
        if frame.len() > wire::MAX_CONTROL_FRAME {
            return Err(io::Error::other(format!(
                "a letter of {} bytes is more than a control message holds ({} bytes)",
                frame.len(),
                wire::MAX_CONTROL_FRAME
            )));
        }
        {
            let mut letters = self.letters();
            super::letters::Outbox::new(&self.dir).put(letters.next, &message)?;
            letters.next += 1;
        }
        self.outbox_changed.notify_one();
        Ok(())
    }

    /// The most a letter to `to` takes as a control message, with the
    /// longest id and count it could carry.
    pub(super) fn letter_size(&self, to: &MachineName, account: &AccountName, letter: &Letter) -> usize {
        let message = self.letter(to, account, Purpose::Words, u64::MAX, LetterId::longest(), letter.clone());
        wire::encode(&Up::Send { seq: u64::MAX, message }).len()
    }

    fn letter(
        &self,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        n: u64,
        id: LetterId,
        letter: Letter,
    ) -> Envelope {
        Envelope {
            hub: self.hub.clone(),
            from: self.me(),
            to: to.clone(),
            n,
            kind: Kind::Letter,
            account: Some(account.clone()),
            purpose: Some(purpose),
            re: None,
            stream: None,
            body: serde_json::to_value(LetterBody { id, letter }).expect("letters serialize"),
        }
    }
}

/// Removes a call's waiter when the call is over, tells the hub the caller
/// gave up when it ends without an answer, and stops what the call does
/// beside waiting.
struct Waiting {
    inner: Arc<Inner>,
    id: u64,
    via: Via,
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
            self.via.cancel(self.id);
        }
    }
}
