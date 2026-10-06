//! The link's connections: dialing the hub, the welcome, and everything
//! that passes on one connection until it ends; then dialing again.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use jiff::Timestamp;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::letters::{self, Outbox, Receipt};
use super::outgoing::Outgoing;
use super::{CallEvent, Connection, Inner, Status};
use crate::dial;
use crate::envelope::{self, Kind};
use crate::message::LetterBody;
use crate::seal::Sealed;
use crate::sign::{Domain, Signed, sha256};
use crate::wire::{self, Down, HEARTBEAT, Intent, Letters, PROTOCOL, Refusal, SILENCE, Up};

/// A connection that stays up this long counts as healthy: the next failure
/// is tried again at once.
const STABLE: Duration = Duration::from_secs(60);

enum Ended {
    /// Try again.
    Retry(String),
    /// The hub refused this machine for good.
    Stop(String),
}

pub(super) async fn run(inner: Arc<Inner>, mut changes: Option<super::changes::Changes>) {
    let mut failures: u32 = 0;
    loop {
        let mut online_since = None;
        let ended = tokio::select! {
            ended = connect(&inner, &mut online_since) => ended,
            changed = changed(&inner, &mut changes) => {
                let reason = changed.expect("watching network and resume notifications");
                reconnect(&inner, reason);
                failures = 0;
                continue;
            }
        };
        match ended {
            Ended::Stop(reason) => {
                inner.node.log(format!("the link to the hub stopped: {reason}"));
                inner.status.send_replace(Status::Stopped { reason });
                return;
            }
            Ended::Retry(reason) => {
                let was_online = matches!(*inner.status.borrow(), Status::Online { .. });
                if online_since.is_some_and(|since| since.elapsed() >= STABLE) {
                    failures = 0;
                }
                failures = failures.saturating_add(1);
                if was_online || failures == 1 {
                    inner.node.log(format!("the link to the hub is down: {reason}"));
                }
                let since = match &*inner.status.borrow() {
                    Status::Connecting { since, .. } if !was_online => *since,
                    _ => Timestamp::now(),
                };
                inner.status.send_replace(Status::Connecting { since, last_error: Some(reason) });
                tokio::select! {
                    _ = tokio::time::sleep(backoff(failures)) => {},
                    changed = changed(&inner, &mut changes) => {
                        reconnect(&inner, changed.expect("watching network and resume notifications"));
                        failures = 0;
                    }
                }
            }
        }
    }
}

async fn changed(inner: &Inner, changes: &mut Option<super::changes::Changes>) -> std::io::Result<&'static str> {
    let platform = async {
        match changes {
            Some(changes) => changes.changed().await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = platform => result,
        () = inner.reconnect.notified() => Ok("the application reported a network change or resume"),
    }
}

fn reconnect(inner: &Inner, reason: &str) {
    inner.node.log(format!("reconnecting immediately: {reason}"));
    inner.status.send_replace(Status::Connecting { since: Timestamp::now(), last_error: Some(reason.to_string()) });
    dial::network_changed();
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

async fn connect(inner: &Arc<Inner>, online_since: &mut Option<Instant>) -> Ended {
    let membership = &inner.membership;
    let dialed = match dial::dial(&membership.hub, membership.cert, wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME).await {
        Ok(dialed) => dialed,
        Err(error) => return Ended::Retry(error),
    };
    let (address, route) = (dialed.address, dialed.route.clone());
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
    match tokio::time::timeout(SILENCE, dial::send(&mut socket, &hello)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Ended::Retry(error),
        Err(_) => return Ended::Retry("sending the hello to the hub timed out".to_string()),
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
                return Ended::Retry(format!(
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
    let (up, mut outgoing) = Outgoing::channel();
    *inner.connection.lock().expect("never poisoned") = Some(Connection { generation, up: up.clone() });
    inner.status.send_replace(Status::Online { since: Timestamp::now(), address, route: route.clone() });
    *online_since = Some(Instant::now());
    inner.node.log(format!("connected to the hub at {address}, {route}"));

    // This guard also runs when a network event cancels connect(). Pending
    // callers must learn that their old connection ended before another
    // connection can be published.
    let mut connected = Connected { inner, generation, reason: "the connection was interrupted".to_string() };

    let (mut sink, mut stream) = socket.split();
    let (writer_done, mut writer_stopped) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let _done = writer_done;
        while let Some(frame) = outgoing.recv().await {
            if sink.send(WsMessage::Binary(wire::encode(&frame.value).into())).await.is_err() {
                return;
            }
            if let Some(written) = frame.written {
                let _ = written.send(());
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
            // A member of a World older than sealing gives the hub its
            // sealing key the first time it connects.
            if let Err(error) = inner.republish().await {
                inner.node.log(format!("giving the hub this machine's sealing key: {error}"));
            }
            inner.node.members_changed();
            // Whatever the towns' ledgers gained while this machine was away
            // is taken now; the entries checked against the members just
            // asked for.
            inner.node.ledger_changed(None);
        }));
    }

    let mut last_pong = Instant::now();
    let mut last_ping = 0;
    let ended = loop {
        let next = tokio::select! {
            // A continuous stream of unrelated frames must not postpone
            // the heartbeat deadline once it has expired.
            biased;
            _ = tokio::time::sleep_until(last_pong + SILENCE) => {
                break Ended::Retry(format!("the hub did not answer a heartbeat for {} s", SILENCE.as_secs()));
            },
            _ = &mut writer_stopped => break Ended::Retry("writing to the hub failed".to_string()),
            failed = &mut letters_failed => break Ended::Retry(failed.unwrap_or_else(|_| "taking letters stopped".to_string())),
            next = stream.next() => next,
        };
        let frame: Down = match next {
            None | Some(Ok(WsMessage::Close(_))) => {
                break Ended::Retry("the hub closed the connection".to_string());
            }
            Some(Err(error)) => break Ended::Retry(dial::describe(&error)),
            Some(Ok(WsMessage::Binary(bytes))) => match serde_json::from_slice(&bytes) {
                Ok(frame) => frame,
                Err(error) => {
                    break Ended::Retry(format!("an unreadable frame from the hub: {error}"));
                }
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
                tokio::spawn(super::answering::answer(inner.clone(), up.clone(), call, message));
            }
            Down::Status { id, status } => {
                if let Some(waiter) = inner.calls.lock().expect("never poisoned").get(&id) {
                    let _ = waiter.events.send(CallEvent::Status(status));
                }
            }
            Down::Answered { id, message } => {
                if let Some(waiter) = inner.calls.lock().expect("never poisoned").get(&id) {
                    let _ = waiter.events.send(CallEvent::Answered(message));
                }
            }
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
                    inner.node.members_changed();
                });
            }
            Down::LedgerChanged { town } => inner.node.ledger_changed(Some(town)),
            Down::Refused { reason } => break refused(reason),
            Down::Welcome { .. } | Down::Joined { .. } | Down::Paired => {
                break Ended::Retry("the hub sent a frame that belongs to another kind of connection".to_string());
            }
        }
    };

    drop(tasks);
    drop(up);
    connected.reason = match &ended {
        Ended::Retry(reason) | Ended::Stop(reason) => reason.clone(),
    };
    if matches!(ended, Ended::Stop(_)) {
        // Publish the authenticated revocation before dropping pending
        // questions. When leaving, the hub may close the revoked session
        // before the separate acknowledgement of the revocation arrives.
        inner.status.send_replace(Status::Stopped { reason: connected.reason.clone() });
    }
    ended
}

struct Connected<'a> {
    inner: &'a Inner,
    generation: u64,
    reason: String,
}

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        let mut connection = self.inner.connection.lock().expect("never poisoned");
        if connection.as_ref().is_some_and(|connection| connection.generation == self.generation) {
            *connection = None;
        }
        drop(connection);
        for waiter in self.inner.calls.lock().expect("never poisoned").values() {
            let _ = waiter.events.send(CallEvent::Disconnected(self.reason.clone()));
        }
        self.inner.asks.lock().expect("never poisoned").clear();
    }
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
        Refusal::Revoked { .. } => Ended::Stop(reason.to_string()),
        other => Ended::Retry(other.to_string()),
    }
}

async fn heartbeat(up: Outgoing) {
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
async fn flush(inner: Arc<Inner>, up: Outgoing, mut sent: u64) {
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
    up: Outgoing,
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
    let claimed = match envelope::claim(message) {
        Ok(claimed) => claimed,
        Err(error) => return set_aside(format!("it cannot be read: {error}")),
    };
    if claimed.to != inner.membership.name || claimed.world != inner.membership.world {
        return set_aside(format!("it is for {} in {}", claimed.to, claimed.world));
    }
    let Some(key) = inner.key_of(&claimed.from).await else {
        return set_aside(format!("{} is not a member this machine trusts", claimed.from));
    };
    let opened = match envelope::open(message, &key) {
        Ok(opened) => opened,
        Err(error) => return set_aside(format!("from {}: {error}", claimed.from)),
    };
    if opened.kind != Kind::Letter {
        return set_aside(format!("from {}: it is not a letter", opened.from));
    }
    let info = super::calls::letter_info(&opened.world, &opened.from, &opened.to);
    let plaintext = match Sealed::from_bytes(&opened.sealed.0)
        .and_then(|sealed| inner.seal.open(&sealed, &info, &opened.header()))
    {
        Ok(plaintext) => plaintext,
        Err(error) => return set_aside(format!("from {}: {error}", opened.from)),
    };
    let LetterBody { id, letter } = match serde_json::from_slice(&plaintext) {
        Ok(body) => body,
        Err(error) => return set_aside(format!("from {}: it cannot be read: {error}", opened.from)),
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
    if let Err(reason) = inner.node.letter(opened.from.clone(), id.clone(), letter).await? {
        set_aside(format!("from {}: {reason}", opened.from))?;
    }
    letters::keep_receipt(&inner.dir, &opened.from, &id, &Receipt { digest, at: Timestamp::now() })
}
