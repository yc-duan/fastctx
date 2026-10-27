//! The link's connections: dialing an upstream, the welcome, and everything
//! that passes on one connection until it ends; then dialing again.
//!
//! The ways to the hub are tried in order: this machine's own line, then
//! through each machine that allows direct connections; one that cannot be
//! reached gives way to the next. A machine on a later way asks the earlier
//! ones now and then whether they take sessions again, and returns to the
//! first that does.

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
use super::{CallEvent, CallOn, Connection, Inner, Replier, Status};
use crate::again::{STABLE, backoff};
use crate::dial::{self, HubSocket, Upstream};
use crate::envelope::{Envelope, Kind};
use crate::message::LetterBody;
use crate::sign::{Domain, sha256};
use crate::wire::{self, Down, HEARTBEAT, Intent, Letters, Refusal, SILENCE, Up};

/// How often a machine on a later way to the hub asks the earlier ones
/// whether they take sessions again: the longest it stays on the later way
/// after its own line is back (`world-network.md`).
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
    'sessions: loop {
        let mut online_since = None;
        let started = Instant::now();
        let ended = {
            let connecting = connect(&inner, &mut online_since);
            tokio::pin!(connecting);
            loop {
                tokio::select! {
                    ended = &mut connecting => break ended,
                    changed = changed(&inner, &mut changes) => {
                        // A session on may well have lived through it: the
                        // hub is asked first, and only no answer ends it
                        // (`world-network.md`, 有人在等，就替他看着线).
                        if matches!(*inner.status.borrow(), Status::Online { .. }) {
                            inner.check.notify_one();
                            continue;
                        }
                        reconnect(&inner, changed);
                        failures = 0;
                        continue 'sessions;
                    }
                }
            }
        };
        match ended {
            Ended::Stop(reason) => {
                inner.node.log(format!("the link to the hub stopped: {reason}"));
                let dissolved = inner.dissolved.load(std::sync::atomic::Ordering::Relaxed);
                let then = *inner.then.lock().expect("never poisoned");
                let ended = super::state::Ended { at: Timestamp::now(), reason: reason.clone(), dissolved, then };
                if let Err(error) = super::state::write(&inner.dir.ended(), &ended) {
                    inner.node.log(format!("keeping that the hub ended this machine's place: {error}"));
                }
                inner.status.send_replace(Status::Stopped { reason });
                inner.node.place_ended(dissolved, then);
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
                let since = match &*inner.status.borrow() {
                    Status::Connecting { since, .. } if !was_online => *since,
                    _ => Timestamp::now(),
                };
                let wait = backoff(failures);
                // Every try leaves a line: which ways it took and why each
                // failed, how long it took and when the next one is, so a
                // long absence shows whether the machine kept trying.
                let next = if wait.is_zero() {
                    "trying again at once".to_string()
                } else {
                    format!("trying again in {:.1} s", wait.as_secs_f64())
                };
                if was_online {
                    inner.node.log(format!("the link to the hub is down: {reason}; {next}"));
                } else {
                    inner.node.log(format!(
                        "connecting to the hub failed (try {failures}, offline since {}, the try took {:.1} s): {reason}; {next}",
                        since.strftime("%Y-%m-%d %H:%M:%S UTC"),
                        started.elapsed().as_secs_f64(),
                    ));
                }
                inner.status.send_replace(Status::Connecting { since, last_error: Some(reason) });
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {},
                    changed = changed(&inner, &mut changes) => {
                        reconnect(&inner, changed);
                        failures = 0;
                    }
                }
            }
        }
    }
}

async fn changed(inner: &Inner, changes: &mut Option<super::changes::Changes>) -> &'static str {
    loop {
        let result = {
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
        };
        match result {
            Ok(reason) => return reason,
            Err(error) => {
                *changes = None;
                inner.node.log(format!("not watching network and resume notifications: {error}"));
            }
        }
    }
}

fn reconnect(inner: &Inner, reason: &str) {
    inner.node.log(format!("reconnecting immediately: {reason}"));
    inner.status.send_replace(Status::Connecting { since: Timestamp::now(), last_error: Some(reason.to_string()) });
    dial::network_changed();
    // The direct links dial again at once too.
    inner.network.send_modify(|changes| *changes = changes.wrapping_add(1));
}

/// The hub was heard: a check asked of it is answered, and counted, so
/// whoever asked for it knows the session on is still heard (`Link::checks`).
fn answered(inner: &Inner, checking: &mut Option<Instant>) {
    if checking.take().is_some() {
        inner.checks.send_modify(|answered| *answered = answered.wrapping_add(1));
    }
}

/// A session the hub welcomed, by one way.
struct Opened {
    socket: HubSocket,
    address: std::net::SocketAddr,
    route: fastctx_types::world::Route,
    taken: u64,
    hub_letters: u64,
    afoot_pending: bool,
}

/// Opens a session by the first way to the hub that takes it, the way that
/// last did tried first. The ways before the one it opens on, in their own
/// order, are asked again while it is on (`ask_preferred`), so a machine on
/// another machine's forwarding goes back to its own line once that works.
async fn connect(inner: &Arc<Inner>, online_since: &mut Option<Instant>) -> Ended {
    let upstreams = inner.upstreams();
    let last = inner.last_way.lock().expect("never poisoned").clone();
    let mut order: Vec<usize> = (0..upstreams.len()).collect();
    if let Some(first) = last.and_then(|last| upstreams.iter().position(|upstream| *upstream == last)) {
        order.retain(|index| *index != first);
        order.insert(0, first);
    }
    let mut failures = Vec::new();
    for index in order {
        let upstream = &upstreams[index];
        match open(inner, upstream).await {
            Ok(opened) => {
                *inner.last_way.lock().expect("never poisoned") = Some(upstream.clone());
                return connected(inner, online_since, &upstreams[..=index], opened).await;
            }
            Err(Ended::Retry(reason)) => failures.push(format!("{upstream}: {reason}")),
            Err(ended) => return ended,
        }
    }
    Ended::Retry(failures.join("; "))
}

async fn open(inner: &Inner, upstream: &Upstream) -> Result<Opened, Ended> {
    let membership = inner.membership();
    let dialed =
        dial::dial_upstream(upstream, &membership.id, membership.cert, wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
            .await
            .map_err(Ended::Retry)?;
    let (address, route) = (dialed.address, dialed.route.clone());
    let mut socket = dialed.socket;
    let proof = inner.key.sign(Domain::Session, &dialed.binding);
    let position = {
        let letters = inner.letters.lock().expect("never poisoned");
        Letters { epoch: letters.file.epoch, taken: letters.file.taken }
    };
    // A computer can take direct connections; it says the addresses of its
    // cards, which the network's administrator chooses its direct address
    // from.
    let computer = membership.kind.is_computer();
    let interfaces = if computer { interface_addresses() } else { Vec::new() };
    let hello = Up::Hello {
        protocol: wire::GREETING_PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Session {
            key: inner.key.public(),
            proof,
            letters: position,
            through: upstream.through().map(ToString::to_string),
            direct: computer,
            interfaces,
            dialed: Some(address),
            any_names: true,
            renames: true,
            afoot: true,
            forgets: true,
            roles: true,
            versions: true,
            due: true,
            project_deletes: true,
            project_entry: true,
            conversation_deletions: true,
            profiles: true,
            account_records: true,
            conversation_plans: true,
            scratch_locations: true,
            left: None,
            cancels: true,
            presence: true,
            parts: true,
            call_details: true,
        },
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
        Down::Welcome { hub, hub_name, name, taken, letters, afoot_pending, .. } => {
            if hub != membership.id {
                return Err(Ended::Retry(format!("the hub is {hub}, not {}", membership.id)));
            }
            if name != membership.name {
                inner.follow_name(name);
            }
            // The hub's short name is the hub's to say: a membership kept
            // before it was known takes it from the welcome.
            if hub_name != membership.hub_name {
                match inner.keep_membership(|kept| kept.hub_name = hub_name.clone()) {
                    Ok(()) => inner.node.log(format!("the hub {hub} is called {hub_name}")),
                    Err(error) => inner.node.log(format!("recording that the hub is called {hub_name}: {error}")),
                }
            }
            Ok(Opened {
                socket,
                address,
                route,
                taken,
                hub_letters: letters,
                afoot_pending: afoot_pending.unwrap_or(false),
            })
        }
        Down::Refused { reason } => Err(refused(inner, reason)),
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
    let Opened { socket, address, route, taken, hub_letters, afoot_pending } = opened;
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

    let generation = super::ways::connection_number();
    inner.afoot_started(generation, afoot_pending);
    let (up, mut outgoing) = Outgoing::channel();
    let (closed, closing) = tokio::sync::watch::channel(None);
    *inner.connection.lock().expect("never poisoned") = Some(Connection {
        generation,
        closed: closing,
        up: up.clone(),
        upstream: upstream.clone(),
        address,
        tried: tried.to_vec(),
    });
    let through = upstream.through().map(ToString::to_string);
    inner.status.send_replace(Status::Online { since: Timestamp::now(), address, through, route: route.clone() });
    inner.ways_changed();
    *online_since = Some(Instant::now());
    match upstream.through() {
        Some(through) => inner.node.log(format!("connected to the hub through {through}, at {address} {route}")),
        None => inner.node.log(format!("connected to the hub at {address} {route}")),
    }
    inner.line_changed(fastctx_types::stats::LineChange::Up { at: Timestamp::now() });

    // This guard also runs when a network event cancels connect(). Pending
    // callers must learn that their old connection ended before another
    // connection can be published.
    let mut guard = Connected { inner, generation, closed, reason: "the connection was interrupted".to_string() };

    let pulse = crate::transport::pulse_of(&socket);
    let (mut sink, mut stream) = socket.split();
    let (writer_done, mut writer_stopped) = oneshot::channel::<()>();
    // No frame goes on being written once nothing of it went, and the hub
    // said it took nothing more, for the silence: a hub that takes nothing
    // is as gone as one that says nothing (`crate::pulse`).
    let writing = pulse.clone();
    let writer = tokio::spawn(async move {
        let _done = writer_done;
        while let Some(frame) = outgoing.recv().await {
            let message = WsMessage::Binary(frame.bytes.into());
            if !writing.writing(sink.send(message)).await.is_ok_and(|sent| sent.is_ok()) {
                return;
            }
            if let Some(written) = frame.written {
                let _ = written.send(());
            }
        }
        let _ = tokio::time::timeout(SILENCE, sink.close()).await;
    });
    let (letter_failed, mut letters_failed) = oneshot::channel::<String>();
    let (outbox_failed, mut outbox_failure) = oneshot::channel::<String>();
    let (deliveries, delivered) = mpsc::channel(DELIVERIES);
    let pinged: Arc<std::sync::Mutex<std::collections::HashMap<u64, Instant>>> = Arc::default();
    let mut tasks = Tasks(vec![
        writer,
        tokio::spawn(heartbeat(inner.clone(), up.clone(), pinged.clone(), pulse.clone())),
        tokio::spawn(flush(inner.clone(), up.clone(), taken, outbox_failed)),
        tokio::spawn(take_letters(inner.clone(), up.clone(), hub_letters, delivered, letter_failed)),
    ]);
    let (returned, mut preferred_back) = oneshot::channel::<Upstream>();
    let mut preferred = tried[..tried.len() - 1].to_vec();
    // On a later address of the hub's own, as one a moved hub's old place
    // forwards from, the earlier ones are asked too.
    if let Upstream::Hub(addresses) = upstream
        && let Some(position) = addresses.iter().position(|dialed| *dialed == address)
        && position > 0
    {
        preferred.push(Upstream::Hub(addresses[..position].to_vec()));
    }
    if !preferred.is_empty() {
        let (hub, cert) = (inner.hub.clone(), inner.cert);
        tasks.0.push(tokio::spawn(ask_preferred(preferred, hub, cert, returned)));
    }
    // What an earlier session was told of who is online is counted by that
    // run of the hub: this session's view and tellings count afresh.
    inner.presences.lock().expect("never poisoned").clear();
    {
        let inner = inner.clone();
        tasks.0.push(tokio::spawn(async move {
            let mut failing = None;
            loop {
                match inner.refresh().await {
                    Ok(()) => break,
                    Err(error) => {
                        if failing.as_ref() != Some(&error) {
                            inner.node.log(format!("asking the hub for the initial view: {error}; trying again"));
                            failing = Some(error);
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
            // The directory and the initial status snapshot complete independently.
            inner.afoot_synced(generation);
            inner.versions_synced(generation);
            tell_worked(&inner).await;
            inner.node.directory_changed();
            inner.node.connected();
            // Whatever the projects' ledgers gained while this machine was
            // away is taken now.
            inner.node.ledger_changed(None);
        }));
    }

    // Anything the hub sends shows it is there, a pong or not: a pong can
    // wait behind letters and answers the hub queued before it.
    let mut heard = Instant::now();
    let mut last_ping = 0;
    // A check asked since the hub was last heard, and how long it waits;
    // and when it was asked.
    let mut checking: Option<Instant> = None;
    let mut asked = Instant::now();

    let ended = loop {
        let next = tokio::select! {
            // Checked first, so that once nothing has come for the whole
            // silence the connection ends even if something is just arriving.
            // A frame still coming in shows the hub is there, as one come
            // whole does.
            biased;
            _ = tokio::time::sleep_until(heard + SILENCE) => {
                if let Some(at) = pulse.since(heard) {
                    heard = at;
                    continue;
                }
                break Ended::Retry(format!("nothing came from the hub for {} s", SILENCE.as_secs()));
            },
            _ = tokio::time::sleep_until(checking.unwrap_or(heard + SILENCE)), if checking.is_some() => {
                if pulse.since(asked).is_some() {
                    checking = None;
                    continue;
                }
                // On another network the way that worked last means
                // nothing: every way is tried again in its own order.
                *inner.last_way.lock().expect("never poisoned") = None;
                break Ended::Reroute("the hub did not answer a check after the network changed".to_string());
            },
            () = inner.check.notified() => {
                if checking.is_none() {
                    asked = Instant::now();
                    checking = Some(asked + inner.check_within());
                    // Numbered apart from the heartbeats, so its pong measures
                    // nothing.
                    let paths = inner.view().direct_links.then(|| inner.paths());
                    let held = Some(inner.letters.lock().expect("never poisoned").file.held);
                    let _ = up.send(Up::Ping { n: 0, paths, took: Some(pulse.taken()), held });
                }
                continue;
            },
            _ = &mut writer_stopped => break Ended::Retry("writing to the hub failed".to_string()),
            () = up.overflowed() => break Ended::Retry("the hub took nothing while frames for it piled up".to_string()),
            () = &mut rerouted => break Ended::Reroute("the ways this machine connects to the hub by changed".to_string()),
            Ok(upstream) = &mut preferred_back => {
                // The next session opens on the way that is back, not again
                // on the one it leaves.
                *inner.last_way.lock().expect("never poisoned") = Some(upstream.clone());
                break Ended::Reroute(format!("{upstream} takes sessions again"));
            },
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
                // A newer hub's frame this machine does not know is passed
                // over: connecting again would hear it again.
                Err(error) => {
                    let kind = serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|frame| frame["f"].as_str().map(str::to_string))
                        .unwrap_or_else(|| "?".into());
                    inner
                        .node
                        .log(format!("passing over a frame from the hub this version cannot read ({kind}): {error}"));
                    heard = Instant::now();
                    continue;
                }
            },
            Some(Ok(message)) => {
                // The hub's own ping, sent when it had nothing else to say,
                // says how much it took of what this machine sent.
                if let WsMessage::Ping(said) = &message
                    && let Some(took) = crate::pulse::said(said)
                {
                    pulse.told(took);
                }
                heard = Instant::now();
                answered(inner, &mut checking);
                continue;
            }
        };
        heard = Instant::now();
        answered(inner, &mut checking);
        match frame {
            Down::Pong { n, at, took, afoot_ready } => {
                if afoot_ready {
                    inner.afoot_complete(generation);
                }
                if let Some(took) = took {
                    pulse.told(took);
                }
                if n > last_ping {
                    last_ping = n;
                    let mut pinged = pinged.lock().expect("never poisoned");
                    if let Some(sent) = pinged.remove(&n) {
                        let ms = super::line::millis(sent.elapsed());
                        *inner.hub_ms.lock().expect("never poisoned") = Some(ms);
                        inner.trip(ms);
                        if let Some(at) = at {
                            inner.heard_time(at, sent.elapsed());
                        }
                    }
                    pinged.retain(|sent, _| *sent > n);
                }
            }
            Down::Ack { seq, refused } => {
                if let Some(reason) = refused
                    && let Err(error) = refused_letter(inner, seq, &reason)
                {
                    break Ended::Retry(format!("recording refused letter {seq}: {error}"));
                }
                if let Err(error) = held(inner, seq) {
                    break Ended::Retry(format!("recording what the hub holds: {error}"));
                }
            }
            // Letters the hub sends faster than this machine takes them wait
            // for it, the connection read no further meanwhile, for as long
            // as a silence lasts at most: an older hub sends all it holds at
            // once.
            Down::Deliver { seq, message } => {
                match tokio::time::timeout(SILENCE, deliveries.send((seq, message))).await {
                    Ok(_) => {}
                    Err(_) => {
                        break Ended::Retry(format!(
                            "the hub's letters came faster than this machine takes them, for {} s",
                            SILENCE.as_secs()
                        ));
                    }
                }
            }
            Down::Incoming { call, message } => {
                let stop = inner.answers.open(inner, CallOn::Hub(generation), call);
                tokio::spawn(super::answering::answer(inner.clone(), Replier::Hub(up.clone(), call), message, stop));
            }
            Down::Cancelled { call } => {
                inner.answers.stop(&CallOn::Hub(generation), call, "its caller gave it up");
            }
            Down::Status { id, status } => inner.call_event(&CallOn::Hub(generation), id, CallEvent::Status(status)),
            Down::Answered { id, message } => {
                inner.call_event(&CallOn::Hub(generation), id, CallEvent::Answered(message));
            }
            Down::Told { id, answer } => inner.asks.told(generation, id, answer),
            Down::Part { id, index, total, text } => inner.asks.part(generation, id, index, total, text),
            Down::Presence { machine, presence, rev } => inner.heard_presence(machine, presence, rev),
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
            Down::AccountRecordChanged { changed } => {
                inner.node.account_record_changed(&changed);
                let _ = inner.record_changes.send(Some(changed));
            }
            Down::Afoot { machine, busy, change } => inner.heard_afoot(machine, busy, *change),
            Down::Forget { forgetting } => {
                let (inner, up) = (inner.clone(), up.clone());
                tokio::spawn(async move {
                    let id = forgetting.id;
                    let outcome = inner.node.forget(forgetting).await;
                    // A session ended meanwhile: the hub tells it again.
                    let _ = up.send(Up::Forgot { id, outcome });
                });
            }
            Down::Versions { network } => inner.heard_versions(generation, *network),
            Down::Fetch { fetch } => inner.fetch_for_hub(generation, *fetch),
            Down::FetchCancelled { id } => inner.cancel_fetch(generation, id),
            Down::PackageProgress { id, progress } => inner.asks.progress(generation, id, progress),
            Down::Refused { reason } => break refused(inner, reason),
            Down::Welcome { .. }
            | Down::LoginResponse { .. }
            | Down::TokenInfo { .. }
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
    closed: tokio::sync::watch::Sender<Option<String>>,
    reason: String,
}

impl Drop for Connected<'_> {
    fn drop(&mut self) {
        self.inner
            .line_changed(fastctx_types::stats::LineChange::Down { at: Timestamp::now(), why: self.reason.clone() });
        let mut connection = self.inner.connection.lock().expect("never poisoned");
        let ended = connection.as_ref().is_some_and(|connection| connection.generation == self.generation);
        if ended {
            *connection = None;
        }
        drop(connection);
        // The way through the hub is gone before the calls on it hear so:
        // whoever pauses to try again sees it closed, not still there.
        if ended {
            self.inner.ways_changed();
        }
        self.inner.fail_calls_on(&CallOn::Hub(self.generation), &self.reason);
        // Nobody hears the answers to the calls that came on it.
        self.inner.answers.stop_all(&CallOn::Hub(self.generation), "the connection to the hub it came on ended");
        self.inner.asks.end(self.generation);
        self.inner.cancel_fetches(self.generation);
        self.closed.send_replace(Some(self.reason.clone()));
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

fn refused(inner: &Inner, reason: Refusal) -> Ended {
    match reason {
        Refusal::Removed { dissolved, then, .. } => {
            inner.dissolved.store(dissolved, std::sync::atomic::Ordering::Relaxed);
            *inner.then.lock().expect("never poisoned") = then;
            Ended::Stop(reason.to_string())
        }
        // Connected again at once, under the name the hub gives.
        Refusal::Renamed { .. } => Ended::Reroute(reason.to_string()),
        other => Ended::Retry(other.to_string()),
    }
}

/// Asks the ways before the one this machine is on, every `RETURN`, whether
/// they take sessions; says the first that does.
async fn ask_preferred(
    preferred: Vec<Upstream>,
    hub: crate::ids::HubId,
    cert: crate::ids::CertPin,
    returned: oneshot::Sender<Upstream>,
) {
    loop {
        tokio::time::sleep(RETURN).await;
        for upstream in &preferred {
            if dial::probe(upstream, &hub, cert).await.is_ok() {
                let _ = returned.send(upstream.clone());
                return;
            }
        }
    }
}

/// Sends a heartbeat every `HEARTBEAT`, noting when, and with it how this
/// machine's lines go, to a hub that keeps that, how much this machine took
/// of what the hub sent (`crate::pulse`), and how far the hub's receipts of
/// its letters are taken in, so the hub tells no refusal again.
async fn heartbeat(
    inner: Arc<Inner>,
    up: Outgoing,
    pinged: Arc<std::sync::Mutex<std::collections::HashMap<u64, Instant>>>,
    pulse: Arc<crate::pulse::Pulse>,
) {
    let mut n = 0;
    loop {
        tokio::time::sleep(HEARTBEAT).await;
        n += 1;
        let paths = inner.view().direct_links.then(|| inner.paths());
        pinged.lock().expect("never poisoned").insert(n, Instant::now());
        let held = Some(inner.letters.lock().expect("never poisoned").file.held);
        if up.send(Up::Ping { n, paths, took: Some(pulse.taken()), held }).is_err() {
            return;
        }
    }
}

/// Tells the hub what this machine noted of the calls it made directly,
/// as the hub notes of the calls it passes on.
pub(super) async fn tell_worked(inner: &Arc<Inner>) {
    if !inner.view().direct_links {
        return;
    }
    for worked in inner.direct.take_worked() {
        let question = wire::Question::Worked {
            to: worked.to.clone(),
            account: worked.account.clone(),
            purpose: worked.purpose,
            resident: worked.resident.clone(),
        };
        // Kept for the next session only when this one ended before the hub
        // answered; a refusal is the hub's answer, and the same report would
        // be refused again.
        let Some(connection) = inner.connected().await else {
            inner.direct.note_worked(worked);
            continue;
        };
        match inner.ask_on(connection, question).await {
            Ok(Some(_)) => {}
            Ok(None) => inner.direct.note_worked(worked),
            Err(refusal) => {
                inner.node.log(format!("the hub refused the note of a direct call to {}: {refusal}", worked.to));
            }
        }
    }
}

/// The addresses of this machine's network cards that others could reach.
fn interface_addresses() -> Vec<std::net::IpAddr> {
    match crate::route::interfaces() {
        Ok(interfaces) => interfaces
            .iter()
            .flat_map(|interface| {
                interface
                    .ipv4
                    .iter()
                    .map(|address| std::net::IpAddr::V4(*address))
                    .chain(interface.ipv6.iter().map(|address| std::net::IpAddr::V6(*address)))
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Records that the hub holds this machine's letters through `seq`, and
/// wakes the sending of the next ones.
fn held(inner: &Inner, seq: u64) -> std::io::Result<()> {
    // Kept before the letters are deleted: a stop in between leaves letters
    // the next start deletes, never a number given out again to a letter
    // the hub would take for one it holds.
    let before = {
        let mut letters = inner.letters.lock().expect("never poisoned");
        let before = letters.file.held;
        if seq <= before {
            return Ok(());
        }
        letters.file.held = seq;
        letters.next = letters.next.max(seq + 1);
        if let Err(error) = super::state::write(&inner.dir.letters(), &letters.file) {
            letters.file.held = before;
            return Err(error);
        }
        before
    };
    Outbox::new(&inner.dir).held_between(before, seq)?;
    inner.outbox_changed.notify_one();
    Ok(())
}

fn refused_letter(inner: &Inner, seq: u64, reason: &str) -> std::io::Result<()> {
    super::posting::refused(inner, seq, reason)?;
    let outbox = Outbox::new(&inner.dir);
    match outbox.get(seq)? {
        Some(message) => {
            let path = letters::record_failure(&inner.dir, "out", reason, &message, |line| inner.node.log(line))?;
            inner.node.log(format!("the hub refused letter {seq}: {reason}; kept in {}", path.display()));
        }
        None => inner.node.log(format!("the hub refused letter {seq}: {reason}")),
    }
    Ok(())
}

/// The most letters this machine has sent the hub that the hub has not yet
/// said it holds, and the most bytes of them: the rest wait their turn on
/// disk, so a long backlog never fills the connection.
const WINDOW: usize = 256;
const WINDOW_BYTES: usize = 8 * 1024 * 1024;

/// How many of the hub's letters are taken before the hub is told, at once,
/// that they are.
const TAKEN_AT_ONCE: usize = 64;

/// How many of the hub's letters may wait to be taken: more than a hub of
/// this release sends at once, which it never fills.
const DELIVERIES: usize = 1024;

/// Sends the letters in the outbox past `sent`, in order, a window at a
/// time, and each new one as there is room for it. Letters are numbered one
/// after another, so the next is known without listing the outbox; one not
/// there was held by the hub, or set aside, meanwhile.
async fn flush(inner: Arc<Inner>, up: Outgoing, mut sent: u64, failed: oneshot::Sender<String>) {
    let outbox = Outbox::new(&inner.dir);
    let mut on_way: std::collections::VecDeque<(u64, usize)> = std::collections::VecDeque::new();
    loop {
        // Asked for before looking: a letter put, or the hub holding some,
        // between the two wakes it all the same.
        let changed = inner.outbox_changed.notified();
        let (held, next) = {
            let letters = inner.letters.lock().expect("never poisoned");
            (letters.file.held, letters.next)
        };
        while on_way.front().is_some_and(|(seq, _)| *seq <= held) {
            on_way.pop_front();
        }
        sent = sent.max(held);
        while sent + 1 < next
            && on_way.len() < WINDOW
            && on_way.iter().map(|(_, size)| size).sum::<usize>() < WINDOW_BYTES
        {
            let seq = sent + 1;
            match outbox.get_sized(seq) {
                Ok(Some((message, size))) => {
                    if up.send(Up::Send { seq, message }).is_err() {
                        return;
                    }
                    on_way.push_back((seq, size));
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = failed.send(format!("reading letter {seq} from the outbox: {error}"));
                    return;
                }
            }
            sent = seq;
        }
        changed.await;
    }
}

/// Takes the hub's letters in the order the hub numbered them, and tells
/// the hub they are taken, once for each run of them that came together.
async fn take_letters(
    inner: Arc<Inner>,
    up: Outgoing,
    hub_letters: u64,
    mut delivered: mpsc::Receiver<(u64, Envelope)>,
    failed: oneshot::Sender<String>,
) {
    while let Some(first) = delivered.recv().await {
        let mut run = vec![first];
        while run.len() < TAKEN_AT_ONCE {
            match delivered.try_recv() {
                Ok(next) => run.push(next),
                Err(_) => break,
            }
        }
        let mut last = None;
        for (seq, message) in run {
            let _receiving = fastctx_platform::doing::doing(format!("receiving letter {seq} from {}", inner.hub));
            let already = inner
                .letters
                .lock()
                .expect("never poisoned")
                .file
                .taken
                .is_some_and(|(epoch, taken)| epoch == hub_letters && seq <= taken);
            if !already {
                // Taken whole, its handling and its receipt, whatever becomes
                // of this session, and one at a time with the next session's:
                // a letter sent again meanwhile waits for it, and finds its
                // receipt.
                let taking = inner.taking.lock_owned().await;
                let taker = inner.clone();
                let taken = tokio::spawn(async move {
                    let _doing = fastctx_platform::doing::doing(format!("persisting letter {seq} from {}", taker.hub));
                    taking.adopt();
                    let _taking = taking;
                    take_letter(&taker, &message).await
                })
                .await
                .map_err(|error| std::io::Error::other(error.to_string()))
                .and_then(|taken| taken);
                if let Err(error) = taken {
                    let _ = failed.send(format!("taking a letter: {error}"));
                    return;
                }
            }
            last = Some(seq);
        }
        let Some(seq) = last else { continue };
        // A stop before this is written has the hub send the run again; each
        // letter's receipt keeps it from being taken twice.
        {
            let mut letters = inner.letters.lock().expect("never poisoned");
            if !letters.file.taken.is_some_and(|(epoch, taken)| epoch == hub_letters && seq <= taken) {
                letters.file.taken = Some((hub_letters, seq));
                if let Err(error) = super::state::write(&inner.dir.letters(), &letters.file) {
                    drop(letters);
                    let _ = failed.send(format!("recording a letter taken: {error}"));
                    return;
                }
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
        let path = letters::record_failure(&inner.dir, "in", &reason, message, |line| inner.node.log(line))?;
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
    // Receipts and business delivery now share the admitted computer's identity.
    let sender = caller.key;
    match letters::receipt(&inner.dir, &sender, &caller.names, &id)? {
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
    letters::keep_receipt(&inner.dir, &sender, &id, &Receipt { digest, at: Timestamp::now() })
}
