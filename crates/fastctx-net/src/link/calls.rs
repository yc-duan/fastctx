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

    /// Tells the machine answering the call `id` that its caller gave it
    /// up: through the hub, which tells it in turn, or over the direct link
    /// when the other machine reads that. An answer nobody waits for is
    /// dropped where it arrives.
    fn cancel(&self, id: u64) {
        match self {
            Via::Hub(connection) => {
                let _ = connection.up.send(Up::Cancel { id });
            }
            Via::Peer(conn) if conn.cancels => {
                let _ = conn.send(crate::peer::Frame::Cancel { id });
            }
            Via::Peer(_) => {}
        }
    }

    /// The control connection ending also ends an answer still being fetched
    /// over a separate data connection.
    async fn ended(&self) -> String {
        match self {
            Via::Peer(conn) => conn.ended().await,
            Via::Hub(connection) => {
                let mut closed = connection.closed.clone();
                loop {
                    if let Some(reason) = closed.borrow_and_update().clone() {
                        return reason;
                    }
                    if closed.changed().await.is_err() {
                        return "the connection to the hub ended".to_string();
                    }
                }
            }
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
        feed: Option<Feed>,
    ) -> Result<RemoteResult, CallFailure> {
        // What a name the machine had before it was renamed names is called
        // by its name now.
        let view = self.view();
        if view.is_me(to) {
            return Err(CallFailure::Unreachable { name: to.clone(), reason: "it is this machine".to_string() });
        }
        let to = &view.canonical(to).clone();
        drop(view);
        // How long the answer is waited for, as the call says
        // (`fastctx_types::contract`), from now, a connection to send it on
        // waited for included: past that it is given up, and the machine
        // answering it told so.
        if matches!(call, RemoteCall::Scratch { .. }) && !self.view().scratch_locations {
            return Err(CallFailure::NotAllowed {
                name: to.clone(),
                reason: fastctx_types::scratch::words::OLD_HUB.into(),
            });
        }
        let within = call.contract().within;
        let what = format!("{to}'s answer to {}", call.kind());
        let calling = self.calling(to, account, purpose, call, feed);
        match within {
            Some(within) => match fastctx_platform::doing::outside(what, within, calling).await {
                Ok(answer) => answer,
                Err(late) => Err(CallFailure::Unknown {
                    name: to.clone(),
                    reason: format!("it gave no answer within {} s", late.within.as_secs()),
                }),
            },
            None => calling.await,
        }
    }

    async fn calling(
        self: &Arc<Self>,
        to: &MachineName,
        account: &AccountName,
        purpose: Purpose,
        call: RemoteCall,
        mut feed: Option<Feed>,
    ) -> Result<RemoteResult, CallFailure> {
        // A phone links directly with a machine only while it has calls to
        // it; the first goes through the hub while the link comes up.
        let _demand = self.demand(to);
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
        let plain = envelope(&Body::Call { call: call.clone() }, None);
        let large = feed.is_none()
            && wire::encode(&Up::Call { id: u64::MAX, message: plain.clone() }).len() > wire::MAX_CONTROL_FRAME;
        if large && !self.view().call_details {
            return Err(CallFailure::NotAllowed {
                name: to.clone(),
                reason: "upgrade the hub before sending calls too large for its control connection".into(),
            });
        }
        let via = match if large || matches!(call, RemoteCall::Scratch { .. }) {
            None
        } else {
            self.direct_link_for(to, account, &call)
        } {
            Some(conn) => {
                if let Some(resident) = crate::message::worked_for(&self.me(), &call) {
                    if self.view().project_entry && !self.view().may_act_for(account, to) {
                        // A job may immediately return a project selection. Its
                        // origin must reach the hub before it starts remotely.
                        self.ask(wire::Question::Worked {
                            to: to.clone(),
                            account: account.clone(),
                            purpose,
                            resident: resident.clone(),
                        })
                        .await
                        .map_err(|reason| CallFailure::Unreachable { name: to.clone(), reason })?;
                    } else {
                        self.direct.note_worked(Worked { to: to.clone(), account: account.clone(), purpose, resident });
                        if self.connection().is_some() && self.view().direct_links {
                            let inner = self.clone();
                            tokio::spawn(async move { super::session::tell_worked(&inner).await });
                        }
                    }
                }
                Via::Peer(conn)
            }
            None => match self.connected().await {
                Some(connection) => Via::Hub(connection),
                None => return Err(CallFailure::NotConnected { reason: self.not_connected() }),
            },
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
        } else if !large {
            (plain, Side::None)
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
            let details = Some(crate::call_details::CallDetails::of(&self.me(), &call));
            let message = envelope(&Body::LargeCall { carried, details }, Some(ticket.clone()));
            if wire::encode(&Up::Call { id: u64::MAX, message: message.clone() }).len() > wire::MAX_CONTROL_FRAME {
                return Err(CallFailure::NotAllowed {
                    name: to.clone(),
                    reason: "the call's authorization facts are too large for a control frame".into(),
                });
            }
            let (inner, name, endpoint) = (self.clone(), to.clone(), via.endpoint(self));
            let task = tokio::spawn(async move {
                if let Err(error) = crate::data::source(&endpoint, &inner.key, ticket.stream, &bytes).await {
                    inner.node.log(format!("sending a large call to {name}: {error}"));
                }
            });
            (message, Side::Sending(task))
        };
        // The answer, a large one fetched too, within the one time the call
        // is waited for.
        let reply = self.route(&via, to, n, message, side, &mut feed).await?;
        self.answer_of(&via, to, reply).await
    }

    /// What `reply`, from `to`, says of the call it answers; a large answer
    /// fetched over the data connection it names.
    async fn answer_of(
        self: &Arc<Self>,
        via: &Via,
        to: &MachineName,
        reply: Envelope,
    ) -> Result<RemoteResult, CallFailure> {
        let broken = |reason: String| CallFailure::Unknown { name: to.clone(), reason };
        match reply.kind {
            // A machine refuses a call before taking it in: nothing ran.
            Kind::Refused => {
                let reason: String = reply.body().unwrap_or_else(|_| "it gave no reason".to_string());
                Ok(Err(fastctx_types::ipc::Failure::not_run(format!("{to} refused the call: {reason}"))))
            }
            Kind::Answer => match reply.body::<Body>() {
                Ok(Body::Answer { result }) => Ok(result),
                Ok(Body::LargeAnswer { carried }) => {
                    let bytes = self.fetch(via, to, &reply, &carried).await.map_err(|reason| {
                        broken(format!("its answer was too large for a message, and fetching it failed: {reason}"))
                    })?;
                    // It answered: what it said cannot be read, as a newer
                    // release may say, which is not the same as no answer.
                    serde_json::from_slice(&bytes)
                        .map_err(|error| CallFailure::Unreadable { name: to.clone(), reason: error.to_string() })
                }
                Ok(_) => Err(broken("what came back is not an answer".to_string())),
                Err(_) => Err(CallFailure::Unreadable {
                    name: to.clone(),
                    reason: serde_json::from_value::<Body>(reply.body.clone())
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_default(),
                }),
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
                        let last_seen = last_seen.map(|at| self.local(at));
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

    /// Takes the bytes of a large answer `to` sends over the data connection
    /// its answer names.
    async fn fetch(
        self: &Arc<Self>,
        via: &Via,
        to: &MachineName,
        answer: &Envelope,
        carried: &Carried,
    ) -> Result<crate::reassembly::Buffer, String> {
        let ticket = answer.stream.as_ref().ok_or("the answer names no data connection")?;
        let view = self.view();
        if !view.same(&ticket.source, to) || !view.is_me(&ticket.sink) || ticket.stream != carried.stream {
            return Err(format!("its data connection is for {} to {}", ticket.source, ticket.sink));
        }
        let endpoint = via.endpoint(self);
        tokio::select! {
            biased;
            reason = via.ended() => Err(reason),
            bytes = crate::data::sink(&endpoint, &self.key, carried) => bytes,
        }
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
    ) -> io::Result<super::Posted> {
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
        let posted = {
            let mut letters = self.letters();
            super::letters::Outbox::new(&self.dir).put(letters.next, &message)?;
            let posted = super::Posted { epoch: letters.file.epoch, seq: letters.next };
            letters.next += 1;
            posted
        };
        self.outbox_changed.notify_one();
        Ok(posted)
    }

    /// The most a letter to `to` takes as a control message, with the
    /// longest id and count it could carry.
    pub(super) fn letter_size(&self, to: &MachineName, account: &AccountName, letter: &Letter) -> usize {
        let message = self.letter(to, account, Purpose::Words, u64::MAX, LetterId::longest(), letter.clone());
        let rename_room = 64usize.saturating_sub(message.from.as_str().len());
        wire::encode(&Up::Send { seq: u64::MAX, message }).len() + rename_room
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
