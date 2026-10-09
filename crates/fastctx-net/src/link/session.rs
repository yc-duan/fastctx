//! The link's connections: dialing an upstream, the welcome, and everything
//! that passes on one connection until it ends; then dialing again.
//!
//! The ways to the hub are tried in order: the relays the machine's view
//! lists for it, then the hub itself; one that cannot be reached gives way
//! to the next. A machine on a later way asks the earlier ones now and then
//! whether they take sessions again, and returns to the first that does.

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
use super::{CallEvent, Connection, Inner, Status, Upstream};
use crate::dial::{self, HubSocket};
use crate::envelope::{Envelope, Kind};
use crate::message::LetterBody;
use crate::sign::{Domain, sha256};
use crate::wire::{self, Down, HEARTBEAT, Intent, Letters, PROTOCOL, Refusal, SILENCE, Up};

/// A connection that stays up this long counts as healthy: the next failure
/// is tried again at once.
const STABLE: Duration = Duration::from_secs(60);

/// How often a machine on a later way to the hub asks the earlier ones
/// whether they take sessions again: the longest it stays on the later way
/// after a relay it connects through first is back (`world-network.md`).
const RETURN: Duration = Duration::from_secs(60);

enum Ended {
    /// Try again.
    Retry(String),
    /// Connect again at once, by the first way.
    Reroute(String),
    /// The hub removed this machine.
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
            Ended::Reroute(reason) => {
                inner.node.log(format!("connecting again: {reason}"));
                inner.status.send_replace(Status::Connecting { since: Timestamp::now(), last_error: Some(reason) });
                failures = 0;
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
                    _ = tokio::time::sleep(dial::backoff(failures)) => {},
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

/// A session the hub welcomed, by one way.
struct Opened {
    socket: HubSocket,
    address: std::net::SocketAddr,
    route: fastctx_types::world::Route,
    taken: u64,
    hub_letters: u64,
}

/// Opens a session by the first way to the hub that takes it.
async fn connect(inner: &Arc<Inner>, online_since: &mut Option<Instant>) -> Ended {
    let upstreams = inner.upstreams();
    let mut failures = Vec::new();
    for (index, upstream) in upstreams.iter().enumerate() {
        match open(inner, upstream).await {
            Ok(opened) => return connected(inner, online_since, &upstreams[..=index], opened).await,
            Err(Ended::Retry(reason)) => failures.push(format!("{upstream}: {reason}")),
            Err(ended) => return ended,
        }
    }
    Ended::Retry(failures.join("; "))
}

async fn open(inner: &Inner, upstream: &Upstream) -> Result<Opened, Ended> {
    let membership = &inner.membership;
    let dialed =
        dial::dial(&upstream.addresses, Some(membership.cert), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
            .await
            .map_err(Ended::Retry)?;
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
        Ok(Err(error)) => return Err(Ended::Retry(error)),
        Err(_) => return Err(Ended::Retry("sending the hello timed out".to_string())),
    }
    let welcome = match tokio::time::timeout(SILENCE, dial::recv(&mut socket)).await {
        Ok(Ok(Some(frame))) => frame,
        Ok(Ok(None)) => return Err(Ended::Retry("the connection was closed".to_string())),
        Ok(Err(error)) => return Err(Ended::Retry(error)),
        Err(_) => {
            return Err(Ended::Retry(format!("no welcome came within {} s", SILENCE.as_secs())));
        }
    };
    match welcome {
        Down::Welcome { hub, name, taken, letters, .. } => {
            if hub != membership.world || name != membership.name {
                return Err(Ended::Retry(format!(
                    "the hub knows this machine as {name} on {hub}, not {} on {}",
                    membership.name, membership.world
                )));
            }
            Ok(Opened { socket, address, route, taken, hub_letters: letters })
        }
        Down::Refused { reason } => Err(refused(reason)),
        _ => Err(Ended::Retry("the first frame was not a welcome".to_string())),
    }
}

/// Everything that passes on a session once it is welcomed, until it ends.
async fn connected(
    inner: &Arc<Inner>,
    online_since: &mut Option<Instant>,
    tried: &[Upstream],
    opened: Opened,
) -> Ended {
    let Opened { socket, address, route, taken, hub_letters } = opened;
    let upstream = tried.last().expect("a session is opened by a way").clone();

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

    // Registered before the connection is published, so a change of the
    // ways noticed from then on ends it.
    let rerouted = inner.rerouted.notified();
    tokio::pin!(rerouted);
    rerouted.as_mut().enable();

    let generation = inner.next_id();
    let (up, mut outgoing) = Outgoing::channel();
    *inner.connection.lock().expect("never poisoned") = Some(Connection {
        generation,
        up: up.clone(),
        upstream: upstream.clone(),
        address,
        tried: tried.to_vec(),
    });
    let relay = upstream.relay.clone();
    inner.status.send_replace(Status::Online { since: Timestamp::now(), address, relay, route: route.clone() });
    *online_since = Some(Instant::now());
    inner.node.log(format!("connected to the hub at {address} by {upstream}, {route}"));

    // This guard also runs when a network event cancels connect(). Pending
    // callers must learn that their old connection ended before another
    // connection can be published.
    let mut guard = Connected { inner, generation, reason: "the connection was interrupted".to_string() };

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
    let (outbox_failed, mut outbox_failure) = oneshot::channel::<String>();
    let (deliveries, delivered) = mpsc::unbounded_channel();
    let mut tasks = Tasks(vec![
        writer,
        tokio::spawn(heartbeat(up.clone())),
        tokio::spawn(flush(inner.clone(), up.clone(), taken, outbox_failed)),
        tokio::spawn(take_letters(inner.clone(), up.clone(), hub_letters, delivered, letter_failed)),
    ]);
    let (returned, mut preferred_back) = oneshot::channel::<Upstream>();
    if tried.len() > 1 {
        let cert = inner.membership.cert;
        tasks.0.push(tokio::spawn(ask_preferred(tried[..tried.len() - 1].to_vec(), cert, returned)));
    }
    {
        let inner = inner.clone();
        tasks.0.push(tokio::spawn(async move {
            if let Err(error) = inner.refresh().await {
                inner.node.log(format!("asking the hub for the view: {error}"));
            }
            inner.node.directory_changed();
            // Whatever the projects' ledgers gained while this machine was
            // away is taken now.
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
            () = &mut rerouted => break Ended::Reroute("the ways this machine connects to the hub by changed".to_string()),
            Ok(upstream) = &mut preferred_back => break Ended::Reroute(format!("{upstream} takes sessions again")),
            failed = &mut letters_failed => break Ended::Retry(failed.unwrap_or_else(|_| "taking letters stopped".to_string())),
            failed = &mut outbox_failure => break Ended::Retry(failed.unwrap_or_else(|_| "sending letters stopped".to_string())),
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
            Down::DirectoryChanged => {
                let inner = inner.clone();
                tokio::spawn(async move {
                    if let Err(error) = inner.refresh().await {
                        inner.node.log(format!("asking the hub for the view: {error}"));
                    }
                    inner.node.directory_changed();
                });
            }
            Down::LedgerChanged { town } => inner.node.ledger_changed(Some(town)),
            Down::Refused { reason } => break refused(reason),
            Down::Welcome { .. }
            | Down::LoginResponse { .. }
            | Down::RegisterResponse { .. }
            | Down::LoggedIn { .. }
            | Down::Registered
            | Down::Paired
            | Down::Ready => {
                break Ended::Retry("the hub sent a frame that belongs to another kind of connection".to_string());
            }
        }
    };

    drop(tasks);
    drop(up);
    guard.reason = match &ended {
        Ended::Retry(reason) | Ended::Reroute(reason) | Ended::Stop(reason) => reason.clone(),
    };
    if matches!(ended, Ended::Stop(_)) {
        // Publish the removal before dropping pending questions. When
        // leaving, the hub may close the session before the separate
        // answer to the leaving arrives.
        inner.status.send_replace(Status::Stopped { reason: guard.reason.clone() });
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
        Refusal::Removed { .. } => Ended::Stop(reason.to_string()),
        other => Ended::Retry(other.to_string()),
    }
}

/// Asks the ways before the one this machine is on, every `RETURN`, whether
/// they take sessions; says the first that does.
async fn ask_preferred(preferred: Vec<Upstream>, cert: crate::ids::CertPin, returned: oneshot::Sender<Upstream>) {
    loop {
        tokio::time::sleep(RETURN).await;
        for upstream in &preferred {
            if dial::probe(&upstream.addresses, cert).await.is_ok() {
                let _ = returned.send(upstream.clone());
                return;
            }
        }
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
async fn flush(inner: Arc<Inner>, up: Outgoing, mut sent: u64, failed: oneshot::Sender<String>) {
    let outbox = Outbox::new(&inner.dir);
    loop {
        let numbers = match outbox.numbers() {
            Ok(numbers) => numbers,
            Err(error) => {
                let _ = failed.send(format!("reading the outbox: {error}"));
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
                    let _ = failed.send(format!("reading letter {seq} from the outbox: {error}"));
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
    mut delivered: mpsc::UnboundedReceiver<(u64, Envelope)>,
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

/// Checks one letter and hands it to the node. A letter that cannot be
/// read or taken is set aside with the reason; only a failure of this
/// machine's own disk is an error, and the hub sends the letter again.
async fn take_letter(inner: &Inner, message: &Envelope) -> std::io::Result<()> {
    let set_aside = |reason: String| -> std::io::Result<()> {
        let path = letters::record_failure(&inner.dir, "in", &reason, message)?;
        inner.node.log(format!("a letter was set aside: {reason}; kept in {}", path.display()));
        Ok(())
    };
    if message.kind != Kind::Letter {
        return set_aside(format!("from {}: it is not a letter", message.from));
    }
    let caller = match inner.admit(message).await {
        Ok(caller) => caller,
        Err(reason) => return set_aside(format!("from {}: {reason}", message.from)),
    };
    let purpose = message.purpose.expect("an admitted letter names its purpose");
    let LetterBody { id, letter } = match message.body() {
        Ok(body) => body,
        Err(error) => return set_aside(format!("from {}: {error}", message.from)),
    };
    let digest = crate::sign::Digest(sha256(&serde_json::to_vec(&letter).expect("letters serialize")));
    match letters::receipt(&inner.dir, &caller.machine, &id)? {
        Some(receipt) if receipt.digest == digest => return Ok(()),
        Some(_) => {
            return set_aside(format!(
                "from {}: the id {} was used before for another letter",
                caller.machine,
                id.as_str()
            ));
        }
        None => {}
    }
    let from = caller.machine.clone();
    if let Err(reason) = inner.node.letter(caller, purpose, id.clone(), letter).await? {
        set_aside(format!("from {from}: {reason}"))?;
    }
    letters::keep_receipt(&inner.dir, &from, &id, &Receipt { digest, at: Timestamp::now() })
}
