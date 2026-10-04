//! The link's connections: dialing the hub, the welcome, and everything
//! that passes on one connection until it ends; then dialing again.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::MachineName;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::RemoteResult;
use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::letters::{self, Outbox, Receipt};
use super::{CallEvent, Connection, Inner, Status};
use crate::dial;
use crate::message::{Body, Message};
use crate::sign::{Domain, Signed, sha256};
use crate::wire::{self, Carried, Down, HEARTBEAT, Intent, Letters, PROTOCOL, Refusal, SILENCE, Ticket, Up};

/// A connection that stays up this long counts as healthy: the next failure
/// is tried again at once.
const STABLE: Duration = Duration::from_secs(60);

enum Ended {
    /// Try again.
    Retry(String),
    /// The hub refused this machine for good.
    Stop(String),
}

pub(super) async fn run(inner: Arc<Inner>) {
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        match connect(&inner).await {
            Ended::Stop(reason) => {
                inner.node.log(format!("the link to the hub stopped: {reason}"));
                inner.status.send_replace(Status::Stopped { reason });
                return;
            }
            Ended::Retry(reason) => {
                if started.elapsed() >= STABLE {
                    failures = 0;
                }
                failures += 1;
                let was_online = matches!(*inner.status.borrow(), Status::Online { .. });
                if was_online || failures == 1 {
                    inner.node.log(format!("the link to the hub is down: {reason}"));
                }
                let since = match &*inner.status.borrow() {
                    Status::Connecting { since, .. } if !was_online => *since,
                    _ => Timestamp::now(),
                };
                inner.status.send_replace(Status::Connecting { since, last_error: Some(reason) });
                tokio::time::sleep(backoff(failures)).await;
            }
        }
    }
}

/// The first retry is at once; after that a random wait up to a ceiling that
/// doubles with each failure, to a minute, so machines that lost the hub
/// together do not all return in the same instant.
fn backoff(failures: u32) -> Duration {
    if failures <= 1 {
        return Duration::ZERO;
    }
    let ceiling_ms = 1000_u64.saturating_mul(1 << (failures - 2).min(6)).min(60_000);
    let random = u64::from_le_bytes(crate::sign::random::<8>().unwrap_or([0; 8]));
    Duration::from_millis(250 + random % ceiling_ms.saturating_sub(250).max(1))
}

async fn connect(inner: &Arc<Inner>) -> Ended {
    let membership = &inner.membership;
    let dialed = match tokio::time::timeout(
        SILENCE,
        dial::dial(&membership.hub, membership.cert, wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME),
    )
    .await
    {
        Ok(Ok(dialed)) => dialed,
        Ok(Err(error)) => return Ended::Retry(error),
        Err(_) => {
            return Ended::Retry(format!("the hub did not answer within {} s", SILENCE.as_secs()));
        }
    };
    let address = dialed.address;
    let mut socket = dialed.socket;
    let proof = inner.key.sign(Domain::Session, &dialed.binding);
    let position = {
        let letters = inner.letters.lock().expect("never poisoned");
        Letters { epoch: letters.file.epoch, taken: letters.file.taken }
    };
    let hello = Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Session { key: inner.key.public(), proof, letters: position },
    };
    if let Err(error) = dial::send(&mut socket, &hello).await {
        return Ended::Retry(error);
    }
    let welcome = match tokio::time::timeout(SILENCE, dial::recv(&mut socket)).await {
        Ok(Ok(Some(frame))) => frame,
        Ok(Ok(None)) => return Ended::Retry("the hub closed the connection".to_string()),
        Ok(Err(error)) => return Ended::Retry(error),
        Err(_) => {
            return Ended::Retry(format!("the hub did not welcome this machine within {} s", SILENCE.as_secs()));
        }
    };
    let (taken, hub_letters) = match welcome {
        Down::Welcome { world, name, taken, letters, .. } => {
            if world != membership.world || name != membership.name {
                return Ended::Stop(format!(
                    "the hub knows this machine as {name} in {world}, not {} in {}",
                    membership.name, membership.world
                ));
            }
            (taken, letters)
        }
        Down::Refused { reason } => return refused(reason),
        _ => return Ended::Retry("the hub's first frame was not a welcome".to_string()),
    };

    // The hub holds this machine's letters through `taken`.
    if let Err(error) = held(inner, taken) {
        return Ended::Retry(format!("recording what the hub holds: {error}"));
    }
    // The hub numbers its letters afresh when it lost its records.
    {
        let mut letters = inner.letters.lock().expect("never poisoned");
        if letters.file.taken.is_none_or(|(epoch, _)| epoch != hub_letters) {
            letters.file.taken = Some((hub_letters, 0));
            if let Err(error) = super::state::write(&inner.dir.letters(), &letters.file) {
                return Ended::Retry(format!("recording the hub's letters: {error}"));
            }
        }
    }

    let generation = inner.next_id();
    let (up, mut outgoing) = mpsc::unbounded_channel::<Up>();
    *inner.connection.lock().expect("never poisoned") = Some(Connection { generation, up: up.clone() });
    inner.status.send_replace(Status::Online { since: Timestamp::now(), address });
    inner.node.log(format!("connected to the hub at {address}"));

    let (mut sink, mut stream) = socket.split();
    let (writer_done, mut writer_stopped) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let _done = writer_done;
        while let Some(frame) = outgoing.recv().await {
            if sink.send(WsMessage::Binary(wire::encode(&frame).into())).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });
    let (letter_failed, mut letters_failed) = oneshot::channel::<String>();
    let (deliveries, delivered) = mpsc::unbounded_channel();
    let mut tasks = Tasks(vec![
        writer,
        tokio::spawn(heartbeat(up.clone())),
        tokio::spawn(flush(inner.clone(), up.clone(), taken)),
        tokio::spawn(take_letters(inner.clone(), up.clone(), hub_letters, delivered, letter_failed)),
    ]);
    {
        let inner = inner.clone();
        tasks.0.push(tokio::spawn(async move {
            if let Err(error) = inner.refresh().await {
                inner.node.log(format!("asking the hub for the members: {error}"));
            }
        }));
    }

    let ended = loop {
        let next = tokio::select! {
            next = tokio::time::timeout(SILENCE, stream.next()) => next,
            _ = &mut writer_stopped => break Ended::Retry("writing to the hub failed".to_string()),
            failed = &mut letters_failed => break Ended::Retry(failed.unwrap_or_else(|_| "taking letters stopped".to_string())),
        };
        let frame: Down = match next {
            Err(_) => {
                break Ended::Retry(format!("nothing was heard from the hub for {} s", SILENCE.as_secs()));
            }
            Ok(None | Some(Ok(WsMessage::Close(_)))) => {
                break Ended::Retry("the hub closed the connection".to_string());
            }
            Ok(Some(Err(error))) => break Ended::Retry(dial::describe(&error)),
            Ok(Some(Ok(WsMessage::Binary(bytes)))) => match serde_json::from_slice(&bytes) {
                Ok(frame) => frame,
                Err(error) => {
                    break Ended::Retry(format!("an unreadable frame from the hub: {error}"));
                }
            },
            Ok(Some(Ok(_))) => continue,
        };
        match frame {
            Down::Pong { .. } => {}
            Down::Ack { seq, refused } => {
                if let Some(reason) = refused {
                    refused_letter(inner, seq, &reason);
                }
                if let Err(error) = held(inner, seq) {
                    break Ended::Retry(format!("recording what the hub holds: {error}"));
                }
            }
            Down::Deliver { seq, message } => {
                let _ = deliveries.send((seq, message));
            }
            Down::Incoming { call, message } => {
                tokio::spawn(answer(inner.clone(), up.clone(), call, message));
            }
            Down::Status { id, status } => {
                if let Some(waiter) = inner.calls.lock().expect("never poisoned").get(&id) {
                    let _ = waiter.events.send(CallEvent::Status(status));
                }
            }
            Down::Answered { id, message } => answered(inner, id, message),
            Down::Told { id, answer } => {
                if let Some(asker) = inner.asks.lock().expect("never poisoned").remove(&id) {
                    let _ = asker.send(answer);
                }
            }
            Down::MembersChanged => {
                let inner = inner.clone();
                tokio::spawn(async move {
                    if let Err(error) = inner.refresh().await {
                        inner.node.log(format!("asking the hub for the members: {error}"));
                    }
                });
            }
            Down::Refused { reason } => break refused(reason),
            Down::Welcome { .. } | Down::Joined { .. } | Down::Paired => {
                break Ended::Retry("the hub sent a frame that belongs to another kind of connection".to_string());
            }
        }
    };

    {
        let mut connection = inner.connection.lock().expect("never poisoned");
        if connection.as_ref().is_some_and(|connection| connection.generation == generation) {
            *connection = None;
        }
    }
    drop(tasks);
    drop(up);
    let reason = match &ended {
        Ended::Retry(reason) | Ended::Stop(reason) => reason.clone(),
    };
    // Calls and questions waiting on this connection get no answer on
    // another one.
    for waiter in inner.calls.lock().expect("never poisoned").values() {
        let _ = waiter.events.send(CallEvent::Disconnected(reason.clone()));
    }
    inner.asks.lock().expect("never poisoned").clear();
    ended
}

/// The tasks of one connection, ended with it however it ends, even when
/// the link itself is stopped in the middle.
struct Tasks(Vec<JoinHandle<()>>);

impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

fn refused(reason: Refusal) -> Ended {
    match reason {
        Refusal::Protocol { .. } | Refusal::NotMember | Refusal::Revoked { .. } => Ended::Stop(reason.to_string()),
        other => Ended::Retry(other.to_string()),
    }
}

async fn heartbeat(up: mpsc::UnboundedSender<Up>) {
    let mut n = 0;
    loop {
        tokio::time::sleep(HEARTBEAT).await;
        n += 1;
        if up.send(Up::Ping { n }).is_err() {
            return;
        }
    }
}

/// Records that the hub holds this machine's letters through `seq`.
fn held(inner: &Inner, seq: u64) -> std::io::Result<()> {
    Outbox::new(&inner.dir).held_through(seq)?;
    let mut letters = inner.letters.lock().expect("never poisoned");
    if seq > letters.file.held {
        letters.file.held = seq;
        letters.next = letters.next.max(seq + 1);
        super::state::write(&inner.dir.letters(), &letters.file)?;
    }
    Ok(())
}

fn refused_letter(inner: &Inner, seq: u64, reason: &str) {
    let outbox = Outbox::new(&inner.dir);
    match outbox.get(seq) {
        Ok(Some(message)) => match letters::record_failure(&inner.dir, "out", reason, &message) {
            Ok(path) => inner.node.log(format!("the hub refused letter {seq}: {reason}; kept in {}", path.display())),
            Err(error) => {
                inner.node.log(format!("the hub refused letter {seq}: {reason}; recording that failed: {error}"))
            }
        },
        Ok(None) => inner.node.log(format!("the hub refused letter {seq}: {reason}")),
        Err(error) => inner.node.log(format!("the hub refused letter {seq}: {reason}; reading it failed: {error}")),
    }
}

/// Sends every letter in the outbox past `sent`, in order, and each new one
/// as it is put there.
async fn flush(inner: Arc<Inner>, up: mpsc::UnboundedSender<Up>, mut sent: u64) {
    let outbox = Outbox::new(&inner.dir);
    loop {
        let numbers = match outbox.numbers() {
            Ok(numbers) => numbers,
            Err(error) => {
                inner.node.log(format!("reading the outbox: {error}"));
                return;
            }
        };
        let start = sent;
        for seq in numbers.into_iter().filter(|seq| *seq > start) {
            match outbox.get(seq) {
                Ok(Some(message)) => {
                    if up.send(Up::Send { seq, message }).is_err() {
                        return;
                    }
                    sent = seq;
                }
                // Held by the hub meanwhile.
                Ok(None) => {}
                Err(error) => {
                    inner.node.log(format!("reading letter {seq} from the outbox: {error}"));
                    return;
                }
            }
        }
        inner.outbox_changed.notified().await;
    }
}

/// Takes the hub's letters in the order the hub numbered them, and tells
/// the hub each is taken.
async fn take_letters(
    inner: Arc<Inner>,
    up: mpsc::UnboundedSender<Up>,
    hub_letters: u64,
    mut delivered: mpsc::UnboundedReceiver<(u64, Signed)>,
    failed: oneshot::Sender<String>,
) {
    while let Some((seq, message)) = delivered.recv().await {
        let already = inner
            .letters
            .lock()
            .expect("never poisoned")
            .file
            .taken
            .is_some_and(|(epoch, taken)| epoch == hub_letters && seq <= taken);
        if !already {
            if let Err(error) = take_letter(&inner, &message).await {
                let _ = failed.send(format!("taking a letter: {error}"));
                return;
            }
            let mut letters = inner.letters.lock().expect("never poisoned");
            letters.file.taken = Some((hub_letters, seq));
            if let Err(error) = super::state::write(&inner.dir.letters(), &letters.file) {
                drop(letters);
                let _ = failed.send(format!("recording a letter taken: {error}"));
                return;
            }
        }
        if up.send(Up::Ack { seq }).is_err() {
            return;
        }
    }
}

/// Opens one letter and hands it to the node. A letter that cannot be
/// opened or taken is set aside with the reason; only a failure of this
/// machine's own disk is an error, and the hub sends the letter again.
async fn take_letter(inner: &Inner, message: &Signed) -> std::io::Result<()> {
    let set_aside = |reason: String| -> std::io::Result<()> {
        let path = letters::record_failure(&inner.dir, "in", &reason, message)?;
        inner.node.log(format!("a letter was set aside: {reason}; kept in {}", path.display()));
        Ok(())
    };
    let claimed: Message = match message.claim() {
        Ok(claimed) => claimed,
        Err(error) => return set_aside(format!("it cannot be read: {error}")),
    };
    if claimed.to != inner.membership.name || claimed.world != inner.membership.world {
        return set_aside(format!("it is for {} in {}", claimed.to, claimed.world));
    }
    let Some(key) = inner.key_of(&claimed.from).await else {
        return set_aside(format!("{} is not a member this machine trusts", claimed.from));
    };
    let opened: Message = match message.open(&key, Domain::Message) {
        Ok(opened) => opened,
        Err(error) => return set_aside(format!("from {}: {error}", claimed.from)),
    };
    let Body::Letter { id, letter } = opened.body else {
        return set_aside(format!("from {}: it is not a letter", opened.from));
    };
    let digest = crate::sign::Digest(sha256(&serde_json::to_vec(&letter).expect("letters serialize")));
    match letters::receipt(&inner.dir, &opened.from, &id)? {
        Some(receipt) if receipt.digest == digest => return Ok(()),
        Some(_) => {
            return set_aside(format!(
                "from {}: the id {} was used before for another letter",
                opened.from,
                id.as_str()
            ));
        }
        None => {}
    }
    if let Err(reason) = inner.node.letter(opened.from.clone(), letter).await? {
        set_aside(format!("from {}: {reason}", opened.from))?;
    }
    letters::keep_receipt(&inner.dir, &opened.from, &id, &Receipt { digest, at: Timestamp::now() })
}

/// Checks a call from another machine, has the node do it, and answers.
async fn answer(inner: Arc<Inner>, up: mpsc::UnboundedSender<Up>, call: u64, message: Signed) {
    let claimed: Message = match message.claim() {
        Ok(claimed) => claimed,
        Err(error) => {
            inner.node.log(format!("an unreadable call came through the hub: {error}"));
            return;
        }
    };
    let caller = claimed.from.clone();
    let result: RemoteResult = match check_call(&inner, &message, &claimed).await {
        Ok(remote) => inner.node.call(caller.clone(), remote).await,
        Err(reason) => {
            inner.node.log(format!("refused a call from {caller}: {reason}"));
            Err(Failure::new(format!("{} refused the call: {reason}", inner.membership.name)))
        }
    };
    let n = match inner.counter.next() {
        Ok(n) => n,
        Err(error) => {
            inner.node.log(format!("numbering an answer: {error}"));
            return;
        }
    };
    let sign = |body: Body| {
        let message = Message {
            world: inner.membership.world.clone(),
            from: inner.membership.name.clone(),
            to: caller.clone(),
            n,
            body,
        };
        Signed::new(&inner.key, Domain::Message, &message)
    };
    let small = sign(Body::Answer { call: claimed.n, result: result.clone() });
    let frame = Up::Answer { call, message: small };
    if wire::encode(&frame).len() <= wire::MAX_CONTROL_FRAME {
        let _ = up.send(frame);
        return;
    }
    // Too large for the control connection: the answer says what follows
    // over a data connection.
    let bytes = serde_json::to_vec(&result).expect("answers serialize");
    let stream = match crate::sign::random::<8>() {
        Ok(random) => u64::from_le_bytes(random),
        Err(error) => {
            inner.node.log(format!("choosing a stream for a large answer: {error}"));
            return;
        }
    };
    let ticket = Ticket {
        world: inner.membership.world.clone(),
        stream,
        source: inner.membership.name.clone(),
        sink: caller.clone(),
    };
    let ticket = Signed::new(&inner.key, Domain::Ticket, &ticket);
    let carried =
        Carried { ticket: ticket.clone(), bytes: bytes.len() as u64, sha256: crate::sign::Digest(sha256(&bytes)) };
    let _ = up.send(Up::Answer { call, message: sign(Body::LargeAnswer { call: claimed.n, carried }) });
    if let Err(error) = crate::data::source(&inner.membership, &inner.key, ticket, &bytes).await {
        inner.node.log(format!("sending a large answer to {caller}: {error}"));
    }
}

async fn check_call(
    inner: &Inner,
    message: &Signed,
    claimed: &Message,
) -> Result<fastctx_types::remote::RemoteCall, String> {
    if claimed.to != inner.membership.name || claimed.world != inner.membership.world {
        return Err(format!("it is for {} in {}", claimed.to, claimed.world));
    }
    let key = inner
        .key_of(&claimed.from)
        .await
        .ok_or_else(|| format!("{} is not a member this machine trusts", claimed.from))?;
    let opened: Message = message.open(&key, Domain::Message).map_err(|error| error.to_string())?;
    let Body::Call { call } = opened.body else {
        return Err("it is not a call".to_string());
    };
    match inner.taken.take(&opened.from, opened.n) {
        Ok(Ok(())) => Ok(call),
        Ok(Err(reason)) => Err(reason),
        Err(error) => Err(format!("recording the call: {error}")),
    }
}

/// An answer to one of this machine's calls.
fn answered(inner: &Arc<Inner>, id: u64, message: Signed) {
    let waiter = inner
        .calls
        .lock()
        .expect("never poisoned")
        .get(&id)
        .map(|waiter| (waiter.to.clone(), waiter.n, waiter.events.clone()));
    let claimed: Message = match message.claim() {
        Ok(claimed) => claimed,
        Err(error) => {
            if let Some((_, _, events)) = waiter {
                let _ = events.send(CallEvent::Broken(format!("its answer cannot be read: {error}")));
            }
            return;
        }
    };
    let opened = check_answer(inner, &message, &claimed, waiter.as_ref().map(|(to, _, _)| to));
    let (body, from) = match opened {
        Ok(opened) => (opened.body, opened.from),
        Err(reason) => {
            if let Some((_, _, events)) = waiter {
                let _ = events.send(CallEvent::Broken(reason));
            }
            return;
        }
    };
    match body {
        Body::Answer { call, result } => {
            if let Some((_, n, events)) = waiter {
                let _ = events.send(if call == n {
                    CallEvent::Answer(result)
                } else {
                    CallEvent::Broken(format!("the answer is to call {call}, not {n}"))
                });
            }
        }
        Body::LargeAnswer { call, carried } => {
            // The answer is fetched even when nobody waits for it any more,
            // so the machine sending it is not left holding it.
            let inner = inner.clone();
            tokio::spawn(async move {
                let fetched = fetch(&inner, &from, &carried).await;
                if let Some((_, n, events)) = waiter {
                    let event = match fetched {
                        Ok(_) if call != n => CallEvent::Broken(format!("the answer is to call {call}, not {n}")),
                        Ok(result) => CallEvent::Answer(result),
                        Err(reason) => CallEvent::Broken(format!(
                            "its answer was too large for a message, and fetching it failed: {reason}"
                        )),
                    };
                    let _ = events.send(event);
                }
            });
        }
        Body::Call { .. } | Body::Letter { .. } => {
            if let Some((_, _, events)) = waiter {
                let _ = events.send(CallEvent::Broken("what came back is not an answer".to_string()));
            }
        }
    }
}

fn check_answer(
    inner: &Inner,
    message: &Signed,
    claimed: &Message,
    expected: Option<&MachineName>,
) -> Result<Message, String> {
    if claimed.to != inner.membership.name || claimed.world != inner.membership.world {
        return Err(format!("the answer is for {} in {}", claimed.to, claimed.world));
    }
    if let Some(expected) = expected
        && claimed.from != *expected
    {
        return Err(format!("the answer is from {}, not {expected}", claimed.from));
    }
    let key = inner
        .members()
        .table
        .get(&claimed.from)
        .map(|member| member.key)
        .ok_or_else(|| format!("{} is not a member this machine trusts", claimed.from))?;
    message.open(&key, Domain::Message).map_err(|error| format!("the answer from {}: {error}", claimed.from))
}

async fn fetch(inner: &Inner, from: &MachineName, carried: &Carried) -> Result<RemoteResult, String> {
    let ticket: Ticket = carried.ticket.claim().map_err(|error| format!("the ticket: {error}"))?;
    if ticket.source != *from || ticket.sink != inner.membership.name {
        return Err(format!("the ticket is for {} to {}", ticket.source, ticket.sink));
    }
    let bytes = crate::data::sink(&inner.membership, &inner.key, carried).await?;
    serde_json::from_slice(&bytes).map_err(|error| format!("the answer cannot be read: {error}"))
}
