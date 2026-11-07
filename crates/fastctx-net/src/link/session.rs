//! The link's connections: dialing an upstream, the welcome, and everything
//! that passes on one connection until it ends; then dialing again.
//!
//! The line itself runs on the lines' thread (`crate::threads`): reading and
//! writing frames, heartbeats, the silence that ends it. Every frame it
//! does not need for that is handed, in the order it came, to a task on the
//! node's runtime, which does what it means; what writes to disk goes to a
//! thread meant to block (`specs/world-engine.md`, 连接与判活).
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
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::letters::{self, Outbox};
use super::outgoing::{Outgoing, Writing};
use super::{CallEvent, CallOn, Connection, Inner, Replier, Status};
use crate::again::{STABLE, backoff};
use crate::dial::{self, HubSocket, Upstream};
use crate::envelope::Envelope;
use crate::sign::Domain;
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
                let keeping = inner.clone();
                match inner.blocking(move || super::state::write(&keeping.dir.ended(), &ended)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        inner.node.log(format!("keeping that the hub ended this machine's place: {error}"))
                    }
                    Err(error) => inner.node.log(format!("keeping that the hub ended this machine's place: {error}")),
                }
                inner.status.send_replace(Status::Stopped { reason });
                inner.tell_node(move |node| node.place_ended(dissolved, then));
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

/// How long each way after the first waits before it is dialed too, in its
/// order: the way that took last, or the hub's own, answers first where it can.
const STAGGER: Duration = Duration::from_millis(250);

/// How soon a way that failed is dialed again while others are still being
/// dialed, at first; it doubles after each. A hub restarting refuses for a
/// moment and takes a second later, while a way through another machine that
/// cannot be reached hangs for its whole silence: the hub's own way is not
/// left waiting behind it (11-04: a second's restart kept a machine offline
/// for 61).
const AGAIN: Duration = Duration::from_secs(2);

/// Opens a session by the first way to the hub that takes it, the way that
/// last did tried first. The ways are dialed together, each a little after the
/// one before it, and one that fails is dialed again while others are still
/// being dialed; only the first dialed through says hello, the rest are let go
/// before they do, for two sessions that said it would replace each other.
/// The ways before the one it opens on, in their own order, are asked again
/// while it is on (`ask_preferred`), so a machine on another machine's
/// forwarding goes back to its own line once that works.
async fn connect(inner: &Arc<Inner>, online_since: &mut Option<Instant>) -> Ended {
    let upstreams = inner.upstreams();
    let last = inner.last_way.lock().expect("never poisoned").clone();
    let mut order: Vec<usize> = (0..upstreams.len()).collect();
    if let Some(first) = last.and_then(|last| upstreams.iter().position(|upstream| *upstream == last)) {
        order.retain(|index| *index != first);
        order.insert(0, first);
    }
    let mut failures = Vec::new();
    let Some((index, dialed)) = dial_any(inner, &upstreams, &order, &mut failures).await else {
        return Ended::Retry(failures.join("; "));
    };
    let upstream = &upstreams[index];
    match greet(inner, upstream, dialed).await {
        Ok(opened) => {
            *inner.last_way.lock().expect("never poisoned") = Some(upstream.clone());
            connected(inner, online_since, &upstreams[..=index], opened).await
        }
        Err(Ended::Retry(reason)) => {
            failures.push(format!("{upstream}: {reason}"));
            Ended::Retry(failures.join("; "))
        }
        Err(ended) => ended,
    }
}

/// The first of `upstreams` dialed through, as `connect` dials them; none
/// where each failed, with why in `failures`.
async fn dial_any(
    inner: &Arc<Inner>,
    upstreams: &[Upstream],
    order: &[usize],
    failures: &mut Vec<String>,
) -> Option<(usize, dial::Dialed)> {
    let membership = inner.membership();
    let dial_one = |index: usize, after: Duration| {
        let (upstream, membership) = (&upstreams[index], &membership);
        async move {
            tokio::time::sleep(after).await;
            let dialed = dial::dial_upstream(
                upstream,
                &membership.id,
                membership.cert,
                wire::CONTROL_PATH,
                wire::MAX_CONTROL_FRAME,
            )
            .await;
            (index, dialed)
        }
    };
    let began = Instant::now();
    let mut dialing = futures_util::stream::FuturesUnordered::new();
    for (position, &index) in order.iter().enumerate() {
        dialing.push(dial_one(index, STAGGER * u32::try_from(position).unwrap_or(u32::MAX)));
    }
    let mut again: std::collections::HashMap<usize, Duration> = std::collections::HashMap::new();
    while let Some((index, dialed)) = dialing.next().await {
        match dialed {
            // Those still being dialed are let go with `dialing`, before any
            // of them says hello.
            Ok(dialed) => return Some((index, dialed)),
            Err(reason) => {
                let wait = *again.entry(index).or_insert(AGAIN);
                if !dialing.is_empty() && began.elapsed() + wait < SILENCE {
                    again.insert(index, wait * 2);
                    dialing.push(dial_one(index, wait));
                } else {
                    failures.push(format!("{}: {reason}", upstreams[index]));
                }
            }
        }
    }
    None
}

/// Says hello on a way dialed through, and takes the hub's welcome.
async fn greet(inner: &Arc<Inner>, upstream: &Upstream, dialed: dial::Dialed) -> Result<Opened, Ended> {
    let membership = inner.membership();
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
    let acknowledged = position.taken;
    let hello = Up::Hello {
        protocol: wire::GREETING_PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Session {
            replicated_records: true,
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
            version_parts: true,
            due: true,
            project_deletes: true,
            project_entry: true,
            conversation_deletions: true,
            forgets_accounts: true,
            profiles: true,
            machine_updates: true,
            history_switch: true,
            account_records: true,
            record_keys: computer,
            skill_files: true,
            conversation_plans: true,
            scratch_locations: true,
            left: None,
            cancels: true,
            presence: true,
            parts: true,
            call_details: true,
            view_deltas: true,
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
            // Hello carries the durable receive position. A welcome proves that
            // it reached this hub even if the previous session lost its final Ack.
            if let Some((epoch, sequence)) = acknowledged {
                letters::acknowledge(&inner.dir, epoch, sequence)
                    .await
                    .map_err(|error| Ended::Retry(format!("settling the prior receive position: {error}")))?;
            }
            // What the welcome changes of the membership holds at once, and
            // is kept on disk off the lines' thread; the node hears of a new
            // name on its own runtime.
            if name != membership.name {
                let following = inner.clone();
                let _ = inner.on_node(async move { following.follow_name(name) }).await;
            }
            // The hub's short name is the hub's to say: a membership kept
            // before it was known takes it from the welcome.
            if hub_name != membership.hub_name {
                inner.change_membership(|kept| kept.hub_name = hub_name.clone());
                inner.node.log(format!("the hub {hub} is called {hub_name}"));
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

    // The hub holds this machine's letters through `taken`; and it numbers
    // its letters afresh when it lost its records. Both are kept on disk,
    // off the lines' thread.
    let keeping = inner.clone();
    let kept = inner
        .blocking(move || -> Result<(), String> {
            held(&keeping, taken).map_err(|error| format!("recording what the hub holds: {error}"))?;
            keeping
                .change_letters(|letters| {
                    let fresh = letters.file.taken.is_none_or(|(epoch, _)| epoch != hub_letters);
                    if fresh {
                        letters.file.taken = Some((hub_letters, 0));
                    }
                    fresh
                })
                .map_err(|error| format!("recording the hub's letters: {error}"))?;
            Ok(())
        })
        .await;
    match kept {
        Ok(Ok(())) => {}
        Ok(Err(reason)) => return Ended::Retry(reason),
        Err(error) => return Ended::Retry(format!("recording the hub's letters: {error}")),
    }

    // Registered before the connection is published, so a change of the
    // ways noticed from then on ends it.
    let rerouted = inner.rerouted.notified();
    tokio::pin!(rerouted);
    rerouted.as_mut().enable();

    let generation = super::ways::connection_number();
    inner.afoot_started(generation, afoot_pending);
    let (up, mut outgoing): (Outgoing, Writing) = Outgoing::channel();
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
    let writer = fastctx_platform::task::spawn(async move {
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
    let (letters_taking, mut letters_stopped) = oneshot::channel::<()>();
    let (outbox_failed, mut outbox_failure) = oneshot::channel::<String>();
    let (frames_failed, mut frames_failure) = oneshot::channel::<String>();
    let (deliveries, delivered) = mpsc::channel(DELIVERIES);
    let (frames, arriving) = mpsc::channel(FRAMES);
    // Set once this connection's view is current: letters are admitted by it.
    let (view_ready, view_current) = watch::channel(false);
    let pinged: Arc<std::sync::Mutex<std::collections::HashMap<u64, Instant>>> = Arc::default();
    // The writer and the heartbeat keep the line, here; sending letters,
    // taking the hub's, and doing what its frames say are the node's work.
    let mut tasks = Tasks(vec![
        writer,
        fastctx_platform::task::spawn(heartbeat(inner.clone(), up.clone(), pinged.clone(), pulse.clone())),
        inner.on_node(flush(inner.clone(), up.clone(), taken, outbox_failed)),
        inner.on_node(take_letters(inner.clone(), up.clone(), hub_letters, delivered, view_current, letters_taking)),
        inner.on_node(handle_frames(inner.clone(), generation, up.clone(), deliveries, arriving, frames_failed)),
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
        tasks.0.push(fastctx_platform::task::spawn(ask_preferred(preferred, hub, cert, returned)));
    }
    // What an earlier session was told of who is online is counted by that
    // run of the hub: this session's view and tellings count afresh.
    inner.presences.lock().expect("never poisoned").clear();
    {
        let inner = inner.clone();
        tasks.0.push(inner.clone().on_node(async move {
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
            inner.view_synced.store(generation, std::sync::atomic::Ordering::Release);
            let _ = view_ready.send(true);
            // Records may have changed while disconnected even when the directory
            // itself is identical. Readers reconcile once this view is current.
            inner.history_catalogs.ownership();
            inner.history_catalogs.invalidate(|_| true);
            let _ = inner.record_changes.send(None);
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
    // The hub's answers to the heartbeats, which come as regularly as the
    // hub answers.
    let mut pongs = crate::beats::Beats::new("link.pong", "link.pong.late");
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
                    let held = Some(inner.held_now.load(std::sync::atomic::Ordering::Acquire));
                    let health = inner.view().health.then(fastctx_platform::health::summary);
                    let workbench = inner.node.workbench();
                    let _ = up.send_first(Up::Ping { n: 0, paths, took: Some(pulse.taken()), held, health, workbench });
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
            _ = &mut letters_stopped => break Ended::Retry("taking letters stopped".to_string()),
            failed = &mut outbox_failure => break Ended::Retry(failed.unwrap_or_else(|_| "sending letters stopped".to_string())),
            failed = &mut frames_failure => break Ended::Retry(failed.unwrap_or_else(|_| "doing what the hub's frames say stopped".to_string())),
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
                    pongs.beat();
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
            // Answers to this machine's own questions and calls go straight
            // to whoever waits for them: routing them is all there is to do,
            // and none waits behind the node's other work.
            Down::Cancelled { call } => {
                inner.answers.stop(&CallOn::Hub(generation), call, "its caller gave it up");
            }
            Down::Status { id, status } => inner.call_event(&CallOn::Hub(generation), id, CallEvent::Status(status)),
            Down::Answered { id, message } => {
                inner.call_event(&CallOn::Hub(generation), id, CallEvent::Answered(message));
            }
            Down::Told { id, answer } => inner.asks.told(generation, id, answer),
            Down::Part { id, index, total, text } => {
                if let Some(decode) = inner.asks.part(generation, id, index, total, text) {
                    inner.blocking(move || decode.run());
                }
            }
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
            // The rest is the node's to do, in the order it came. A node that
            // falls this far behind lets the connection go rather than hold
            // the hub's frames without end: the next one starts afresh, and
            // what it missed comes again.
            frame => match frames.try_send(frame) {
                Ok(()) => {
                    fastctx_platform::health::bound("link.frames").note(FRAMES - frames.capacity(), FRAMES);
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    break Ended::Retry(format!(
                        "the hub's frames piled up faster than this machine does what they say ({FRAMES} waiting)"
                    ));
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    break Ended::Retry("doing what the hub's frames say stopped".to_string());
                }
            },
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

/// How many of the hub's frames may wait for the node to do what they say.
/// The hub sends letters a window at a time and bounds the rest it queues
/// for one machine, so a node that keeps up never comes near it.
const FRAMES: usize = 4096;

/// Does what the hub's frames say, in the order they came, on the node's
/// runtime: the line goes on being read and kept while it takes its time.
async fn handle_frames(
    inner: Arc<Inner>,
    generation: u64,
    up: Outgoing,
    deliveries: mpsc::Sender<(u64, Envelope)>,
    mut arriving: mpsc::Receiver<Down>,
    failed: oneshot::Sender<String>,
) {
    let mut version_parts = super::version_parts::Receiving::default();
    while let Some(frame) = arriving.recv().await {
        if let Err(reason) = handle_frame(&inner, generation, &up, &deliveries, &mut version_parts, frame).await {
            let _ = failed.send(reason);
            return;
        }
    }
}

async fn handle_frame(
    inner: &Arc<Inner>,
    generation: u64,
    up: &Outgoing,
    deliveries: &mpsc::Sender<(u64, Envelope)>,
    version_parts: &mut super::version_parts::Receiving,
    frame: Down,
) -> Result<(), String> {
    match frame {
        Down::Ack { seq, refused } => {
            let keeping = inner.clone();
            inner
                .blocking(move || -> Result<(), String> {
                    if let Some(reason) = refused {
                        refused_letter(&keeping, seq, &reason)
                            .map_err(|error| format!("recording refused letter {seq}: {error}"))?;
                    }
                    held(&keeping, seq).map_err(|error| format!("recording what the hub holds: {error}"))
                })
                .await
                .map_err(|error| format!("recording what the hub holds: {error}"))??;
        }
        // Letters the hub sends faster than this machine takes them wait
        // for it, for as long as a silence lasts at most: an older hub
        // sends all it holds at once.
        Down::Deliver { seq, message } => {
            if tokio::time::timeout(SILENCE, deliveries.send((seq, message))).await.is_err() {
                return Err(format!(
                    "the hub's letters came faster than this machine takes them, for {} s",
                    SILENCE.as_secs()
                ));
            }
        }
        Down::Incoming { call, message } => {
            let stop = inner.answers.open(inner, CallOn::Hub(generation), call);
            fastctx_platform::task::spawn(super::answering::answer(
                inner.clone(),
                Replier::Hub(up.clone(), call),
                message,
                stop,
            ));
        }
        Down::Presence { machine, presence, rev } => inner.heard_presence(machine, presence, rev),
        Down::View { delta } => inner.heard_view(*delta),
        Down::DirectoryChanged => {
            let inner = inner.clone();
            fastctx_platform::task::spawn(async move {
                if let Err(error) = inner.refresh().await {
                    inner.node.log(format!("asking the hub for the view: {error}"));
                }
                inner.node.directory_changed();
            });
        }
        Down::HistoryChanged { town, name, version } => {
            inner.history_catalogs.project(&town);
            inner.node.history_changed(town, name, version);
        }
        Down::ReplicasChanged => {
            // Ownership removal can hide its own tombstone from former readers.
            inner.history_catalogs.ownership();
            inner.node.replicas_changed();
        }
        Down::LedgerChanged { town } => inner.node.ledger_changed(Some(town)),
        Down::AccountRecordChanged { changed } => {
            inner.history_catalogs.record(&changed);
            inner.node.account_record_changed(&changed);
            let _ = inner.record_changes.send(Some(changed));
        }
        Down::Afoot { machine, busy, change } => inner.heard_afoot(machine, busy, *change),
        Down::Forget { forgetting } => {
            let (inner, up) = (inner.clone(), up.clone());
            fastctx_platform::task::spawn(async move {
                let id = forgetting.id;
                let outcome = inner.node.forget(forgetting).await;
                // A session ended meanwhile: the hub tells it again.
                let _ = up.send(Up::Forgot { id, outcome });
            });
        }
        Down::ForgetAccount { forgetting } => {
            let (inner, up) = (inner.clone(), up.clone());
            fastctx_platform::task::spawn(async move {
                let id = forgetting.id;
                let outcome = inner.node.forget_account(forgetting).await;
                // A session ended meanwhile: the hub tells it again.
                let _ = up.send(Up::ForgotAccount { id, outcome });
            });
        }
        Down::Versions { network } => {
            version_parts.clear();
            inner.heard_versions(generation, *network);
        }
        Down::VersionPart { index, last, network } => match version_parts.part(index, last, *network) {
            Ok(Some(network)) => inner.heard_versions(generation, network),
            Ok(None) => {}
            Err(error) => return Err(format!("reading network versions: {error}")),
        },
        Down::Fetch { fetch } => inner.fetch_for_hub(generation, *fetch),
        Down::FetchCancelled { id } => inner.cancel_fetch(generation, id),
        // Done on the lines' thread.
        Down::Pong { .. }
        | Down::Cancelled { .. }
        | Down::Status { .. }
        | Down::Answered { .. }
        | Down::Told { .. }
        | Down::Part { .. }
        | Down::PackageProgress { .. }
        | Down::Refused { .. }
        | Down::Welcome { .. }
        | Down::LoginResponse { .. }
        | Down::TokenInfo { .. }
        | Down::RegisterResponse { .. }
        | Down::LoggedIn { .. }
        | Down::Registered
        | Down::Paired
        | Down::Ready => {}
    }
    Ok(())
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
    let mut beats = crate::beats::Beats::from_now("link.heartbeat", "link.heartbeat.late");
    loop {
        tokio::select! {
            () = tokio::time::sleep(HEARTBEAT) => {}
            () = inner.tell.notified() => {
                // Told at once, numbered apart from the heartbeats, so its
                // pong measures nothing.
                let held = Some(inner.held_now.load(std::sync::atomic::Ordering::Acquire));
                let health = inner.view().health.then(fastctx_platform::health::summary);
                let workbench = inner.node.workbench();
                if up.send_first(Up::Ping { n: 0, paths: None, took: Some(pulse.taken()), held, health, workbench }).is_err() {
                    return;
                }
                continue;
            }
        }
        beats.beat();
        n += 1;
        let paths = inner.view().direct_links.then(|| inner.paths());
        pinged.lock().expect("never poisoned").insert(n, Instant::now());
        let held = Some(inner.held_now.load(std::sync::atomic::Ordering::Acquire));
        let health = inner.view().health.then(fastctx_platform::health::summary);
        let workbench = inner.node.workbench();
        if up.send_first(Up::Ping { n, paths, took: Some(pulse.taken()), held, health, workbench }).is_err() {
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
    let mut before = 0;
    let further = inner.change_letters(|letters| {
        before = letters.file.held;
        let further = seq > letters.file.held;
        if further {
            letters.file.held = seq;
            letters.next = letters.next.max(seq + 1);
        }
        further
    })?;
    if !further {
        return Ok(());
    }
    inner.held_now.store(seq, std::sync::atomic::Ordering::Release);
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

/// How many of the hub's letters are taken before the hub is told, at once,
/// that they are.
const TAKEN_AT_ONCE: usize = 64;

/// How many times in a row a letter's handling fails for good, as when the
/// conversation it is for was deleted here, before it is set aside: a
/// couple of minutes at the pace of trying again (`crate::again`).
const SET_ASIDE_AFTER: u32 = 8;

/// How many of the hub's letters may wait to be taken: more than a hub of
/// this release sends at once, which it never fills.
const DELIVERIES: usize = 1024;

/// Sends the letters in the outbox past `sent`, in order, a window at a
/// time (`crate::window`), and each new one as there is room for it: the
/// rest wait their turn on disk, so a long backlog never fills the
/// connection. Letters are numbered one after another, so the next is known
/// without listing the outbox; one not there was held by the hub, or set
/// aside, meanwhile.
async fn flush(inner: Arc<Inner>, up: Outgoing, sent: u64, failed: oneshot::Sender<String>) {
    let outbox = Outbox::new(&inner.dir);
    let mut window = crate::window::Sending::from(sent);
    loop {
        // Asked for before looking: a letter put, or the hub holding some,
        // between the two wakes it all the same.
        let changed = inner.outbox_changed.notified();
        let (held, next) = {
            let letters = inner.letters.lock().expect("never poisoned");
            (letters.file.held, letters.next)
        };
        window.skip_through(held);
        while window.sent() + 1 < next && window.room() {
            let seq = window.sent() + 1;
            // The outbox is on disk, read where reading may block.
            let reading = outbox.clone();
            let read = fastctx_platform::task::spawn_blocking(move || reading.get_sized(seq))
                .await
                .unwrap_or_else(|error| Err(std::io::Error::other(error)));
            match read {
                Ok(Some((message, size))) => {
                    if up.send(Up::Send { seq, message }).is_err() {
                        return;
                    }
                    window.went(seq, Some(size as u64));
                }
                Ok(None) => window.went(seq, None),
                Err(error) => {
                    let _ = failed.send(format!("reading letter {seq} from the outbox: {error}"));
                    return;
                }
            }
        }
        changed.await;
    }
}

/// Takes the hub's letters in the order the hub numbered them, and tells
/// the hub they are taken, once for each run of them that came together.
///
/// None is taken before this connection's view is current: admitting a
/// letter needs it, and a letter refused for want of it would be set aside.
/// Letters wait meanwhile in `delivered`, which holds more than the hub
/// sends ahead. The connection ends once this stops, `_taking` with it.
async fn take_letters(
    inner: Arc<Inner>,
    up: Outgoing,
    hub_letters: u64,
    mut delivered: mpsc::Receiver<(u64, Envelope)>,
    mut view_current: watch::Receiver<bool>,
    _taking: oneshot::Sender<()>,
) {
    if view_current.wait_for(|current| *current).await.is_err() {
        return;
    }
    let mut unsettled = String::new();
    while let Some(first) = delivered.recv().await {
        let mut run = vec![first];
        while run.len() < TAKEN_AT_ONCE {
            match delivered.try_recv() {
                Ok(next) => run.push(next),
                Err(_) => break,
            }
        }
        // What came is kept unacknowledged and tried again on this
        // connection when keeping it fails: the hub holds what follows
        // meanwhile, and calls and heartbeats go on. Ending the connection
        // instead would end it again with each delivery while, say, the
        // conversation a letter is for still moves into its database.
        let run: Arc<[(u64, Envelope)]> = run.into();
        let mut failures = 0;
        let mut said = String::new();
        // The letter the run last stopped at, and how many times in a row:
        // one whose handling keeps failing in a way taking it again does not
        // mend is set aside, so that it holds up none behind it.
        let mut stuck: Option<(u64, u32)> = None;
        let seq = loop {
            let give_up = stuck.filter(|(_, times)| *times >= SET_ASIDE_AFTER).map(|(at, _)| at);
            match letters::take(inner.clone(), hub_letters, run.clone(), give_up).await {
                Ok(seq) => break seq,
                Err(untaken) => {
                    failures += 1;
                    stuck = match (untaken.at, stuck) {
                        (Some(at), Some((was, times))) if at == was => Some((at, times + 1)),
                        (Some(at), _) => Some((at, 1)),
                        (None, _) => None,
                    };
                    let error = untaken.error.to_string();
                    if error != said {
                        inner.node.log(format!("taking letters failed, trying again: {error}"));
                        said = error;
                    }
                    tokio::time::sleep(backoff(failures + 1)).await;
                }
            }
        };
        if failures > 0 {
            inner.node.log(format!("letters were taken after {failures} failed tries"));
        }
        if up.write_first(Up::Ack { seq }).await.is_err() {
            return;
        }
        // Settling is cumulative: one that fails, say on a disk too slow to
        // answer in time, is done by the next, and the hub, which has the
        // acknowledgement, sends none of these again. Ending the connection
        // for it would take the machine off the network on a slow disk.
        match letters::acknowledge(&inner.dir, hub_letters, seq).await {
            Ok(()) => unsettled.clear(),
            Err(error) => {
                let error = error.to_string();
                if error != unsettled {
                    inner.node.log(format!("settling received letters failed, left to the next settling: {error}"));
                    unsettled = error;
                }
            }
        }
    }
}
