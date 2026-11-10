//! Lightweight authenticated members. Only the hub is resource constrained;
//! the load generator does not start full nodes or any model host.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fastctx_net::dial::{self, HubSocket};
use fastctx_net::wire::{self, Change, Down, Intent, Letters, Question, Told, Up};
use tokio::sync::{Semaphore, watch};

use crate::fixture::Identity;
use fastctx_types::MachineName;

#[derive(Default)]
pub struct Stats {
    pub fault: Option<String>,
    pub connected: AtomicUsize,
    pub paused: AtomicUsize,
    pub dialing: AtomicUsize,
    pub peak_dials: AtomicUsize,
    pub handshake_retries: AtomicUsize,
    pub connections: AtomicUsize,
    pub unexpected: AtomicUsize,
    pub stable_lost: AtomicUsize,
    pub views: AtomicUsize,
    pub initially_viewed: AtomicUsize,
    pub initially_ponged: AtomicUsize,
    pub reconnected_viewed: AtomicUsize,
    pub reconnected_ponged: AtomicUsize,
    pub expected_group: AtomicU64,
    pub observed_groups: [AtomicU64; 2],
    pub pending_changes: AtomicUsize,
    pub wire_replays: AtomicUsize,
    pub changes: AtomicUsize,
    pub deltas: AtomicUsize,
    pub letters_sent: AtomicUsize,
    pub letters_held: AtomicUsize,
    pub letters_received: AtomicUsize,
    pub pongs: AtomicUsize,
    pub longest_pong_gap_ms: AtomicU64,
    pub longest_view_ms: AtomicU64,
}

struct State {
    epoch: u64,
    taken: Option<(u64, u64)>,
    held: u64,
    next: u64,
    pending: Option<fastctx_net::envelope::Envelope>,
    checked: crate::validation::Checks,
    viewed: bool,
    ponged: bool,
    presences: HashMap<MachineName, (u64, bool)>,
    view: Option<fastctx_net::directory::View>,
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    index: usize,
    members: usize,
    identity: Arc<Identity>,
    address: SocketAddr,
    pin: fastctx_net::ids::CertPin,
    mut phase: watch::Receiver<u8>,
    slots: Arc<Semaphore>,
    stats: Arc<Stats>,
) -> anyhow::Result<()> {
    let mut state = State {
        epoch: index as u64 + 1,
        taken: None,
        held: 0,
        next: 1,
        pending: None,
        checked: crate::validation::Checks::new(index, members),
        viewed: false,
        ponged: false,
        presences: HashMap::new(),
        view: None,
    };
    let mut attempts = 0_u32;
    loop {
        if *phase.borrow() == 3 {
            return Ok(());
        }
        if !identity.stable && *phase.borrow() == 1 {
            phase.changed().await?;
            continue;
        }
        let permit = slots.acquire().await?;
        let dialing = stats.dialing.fetch_add(1, Ordering::SeqCst) + 1;
        stats.peak_dials.fetch_max(dialing, Ordering::SeqCst);
        let opened = connect(&identity, address, pin, &state).await;
        stats.dialing.fetch_sub(1, Ordering::SeqCst);
        drop(permit);
        let (socket, hub, epoch, held) = match opened {
            Ok(opened) => opened,
            Err(error)
                if !identity.stable
                    && *phase.borrow() == 2
                    && (error.to_string().contains("no answer within 30 s")
                        || error.to_string() == "deadline has elapsed") =>
            {
                attempts += 1;
                stats.handshake_retries.fetch_add(1, Ordering::Relaxed);
                let delay = 250_u64 * (1 << attempts.min(6)) + (index as u64 % 23) * 17;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                continue;
            }
            Err(error) => {
                stats.unexpected.fetch_add(1, Ordering::Relaxed);
                return Err(error);
            }
        };
        if state.taken.is_none_or(|(known, _)| known != epoch) {
            state.taken = Some((epoch, 0));
        }
        if state.pending.is_some() && held >= state.next {
            state.pending = None;
            state.next = held + 1;
            state.held = held;
            stats.letters_held.fetch_add(1, Ordering::Relaxed);
        }
        stats.connected.fetch_add(1, Ordering::SeqCst);
        stats.connections.fetch_add(1, Ordering::SeqCst);
        let result = session(index, &identity, socket, hub, &mut state, &mut phase, &stats).await;
        stats.connected.fetch_sub(1, Ordering::SeqCst);
        if let Err(error) = result {
            stats.unexpected.fetch_add(1, Ordering::Relaxed);
            if identity.stable {
                stats.stable_lost.fetch_add(1, Ordering::Relaxed);
            }
            return Err(error);
        }
    }
}

async fn connect(
    identity: &Identity,
    address: SocketAddr,
    pin: fastctx_net::ids::CertPin,
    state: &State,
) -> anyhow::Result<(HubSocket, fastctx_net::ids::HubId, u64, u64)> {
    let dialed = dial::dial(&[address], Some(pin), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
        .await
        .map_err(anyhow::Error::msg)?;
    let hello = greeting(identity, &dialed.binding, Letters { epoch: state.epoch, taken: state.taken }, address);
    let mut socket = dialed.socket;
    dial::send(&mut socket, &hello).await.map_err(anyhow::Error::msg)?;
    let first =
        tokio::time::timeout(Duration::from_secs(30), dial::recv(&mut socket)).await?.map_err(anyhow::Error::msg)?;
    match first {
        Some(Down::Welcome { hub, name, letters, taken, .. }) if name == identity.name => {
            Ok((socket, hub, letters, taken))
        }
        other => anyhow::bail!("{} received {other:?} instead of its welcome", identity.name),
    }
}

async fn session(
    index: usize,
    identity: &Identity,
    mut socket: HubSocket,
    hub: fastctx_net::ids::HubId,
    state: &mut State,
    phase: &mut watch::Receiver<u8>,
    stats: &Stats,
) -> anyhow::Result<()> {
    let mut beat = tokio::time::interval(Duration::from_secs(10));
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut work = tokio::time::interval(Duration::from_secs(if identity.controller { 2 } else { 5 }));
    work.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n = 1;
    let mut question = 1;
    let mut queried = None;
    let mut last_pong = None;
    let mut pong_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let mut paused = false;
    let mut pong_seen = false;
    let mut view_seen = false;
    let mut last_acknowledged_ping = 0;
    let mut heard = tokio::time::Instant::now();
    let mut last_view = Instant::now() - Duration::from_secs(60);
    if !identity.controller {
        dial::send(
            &mut socket,
            &Up::Afoot { change: Box::new(fastctx_types::town::AfootChange { anew: true, ..Default::default() }) },
        )
        .await
        .map_err(anyhow::Error::msg)?;
    }
    if let Some(message) = &state.pending {
        dial::send(&mut socket, &Up::Send { seq: state.next, message: message.clone() })
            .await
            .map_err(anyhow::Error::msg)?;
    }
    loop {
        tokio::select! {
            changed = phase.changed() => {
                changed?;
                if *phase.borrow() == 4 && !paused {
                    paused = true;
                    stats.paused.fetch_add(1, Ordering::SeqCst);
                }
                if *phase.borrow() == 3 || (!identity.stable && *phase.borrow() == 1) {
                    return Ok(());
                }
            }
            _ = beat.tick() => {
                dial::send(&mut socket, &Up::Ping { n, paths: None, took: None, held: Some(state.held), health: None, workbench: None }).await.map_err(anyhow::Error::msg)?;
                n += 1;
            }
            _ = work.tick() => {
                if identity.controller && matches!(*phase.borrow(), 0 | 2) {
                    let change = Change::Group { machine: "m0".parse().map_err(anyhow::Error::msg)?, group: Some(format!("group-{question}")) };
                    dial::send(&mut socket, &Up::Ask { id: question, question: Question::Manage { account: identity.account.clone(), change } }).await.map_err(anyhow::Error::msg)?;
                    stats.pending_changes.fetch_add(1, Ordering::Relaxed);
                    question += 1;
                } else if !identity.controller && queried.is_none() && last_view.elapsed() >= Duration::from_secs(30) {
                    queried = Some((question, Instant::now()));
                    dial::send(&mut socket, &Up::Ask { id: question, question: Question::Directory }).await.map_err(anyhow::Error::msg)?;
                    question += 1;
                    last_view = Instant::now();
                }
                if matches!(*phase.borrow(), 0 | 2) && !identity.controller && index.is_multiple_of(100) && state.pending.is_none() {
                    let message = fastctx_net::envelope::Envelope {
                        hub: hub.clone(), from: identity.name.clone(), to: identity.peer.clone(),
                        account: Some(identity.account.clone()), purpose: Some(fastctx_types::access::Purpose::Echo),
                        kind: fastctx_net::envelope::Kind::Letter, n: state.next, re: None, stream: None,
                        body: serde_json::to_value(fastctx_net::message::LetterBody {
                            id: format!("scale-{index}-{}", state.next).try_into().map_err(anyhow::Error::msg)?,
                            letter: fastctx_types::remote::Letter::Left { resident: "codex-7f3a".parse().map_err(anyhow::Error::msg)? },
                        })?,
                    };
                    stats.letters_sent.fetch_add(1, Ordering::Relaxed);
                    state.pending = Some(message.clone());
                    dial::send(&mut socket, &Up::Send { seq: state.next, message }).await.map_err(anyhow::Error::msg)?;
                }
            }
            frame = dial::recv(&mut socket) => {
                heard = tokio::time::Instant::now();
                match frame.map_err(anyhow::Error::msg)?.ok_or_else(|| anyhow::anyhow!("{} disconnected", identity.name))? {
                    Down::Pong { n: answer, .. } if answer > last_acknowledged_ping && answer < n => {
                        if index == 0 && stats.fault.as_deref() == Some("pong") { continue; }
                        last_acknowledged_ping = answer;
                        pong_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
                        if !pong_seen {
                            if !state.ponged { stats.initially_ponged.fetch_add(1, Ordering::Relaxed); state.ponged = true; }
                            else if !identity.stable { stats.reconnected_ponged.fetch_add(1, Ordering::Relaxed); }
                            pong_seen = true;
                        }
                        if let Some(previous) = last_pong { stats.longest_pong_gap_ms.fetch_max(Instant::now().duration_since(previous).as_millis() as u64, Ordering::Relaxed); }
                        last_pong = Some(Instant::now()); stats.pongs.fetch_add(1, Ordering::Relaxed);
                    }
                    Down::Told { id, answer: Ok(Told::Directory { mut view }) } => {
                        for (name, (revision, online)) in &state.presences {
                            if *revision > view.presence && let Some(member) = view.directory.members.iter_mut().find(|member| member.name == *name) { Arc::make_mut(member).online = *online; }
                        }
                        state.checked.view(identity, &view, stats)?;
                        state.view = Some(*view);
                        if !view_seen {
                            if !state.viewed { stats.initially_viewed.fetch_add(1, Ordering::Relaxed); state.viewed = true; }
                            else if !identity.stable { stats.reconnected_viewed.fetch_add(1, Ordering::Relaxed); }
                            view_seen = true;
                        }
                        if let Some((asked, began)) = queried && asked == id {
                            stats.longest_view_ms.fetch_max(began.elapsed().as_millis() as u64, Ordering::Relaxed);
                            stats.views.fetch_add(1, Ordering::Relaxed); queried = None;
                        }
                    }
                    Down::View { delta } => {
                        if let Some(view) = &state.view && view.revision == delta.base {
                            let next = fastctx_net::view_delta::apply(view, &delta).map_err(anyhow::Error::msg)?.0;
                            state.checked.view(identity, &next, stats)?;
                            state.view = Some(next);
                            stats.deltas.fetch_add(1, Ordering::Relaxed);
                        } else { last_view = Instant::now() - Duration::from_secs(60); }
                    }
                    Down::DirectoryChanged => { last_view = Instant::now() - Duration::from_secs(60); }
                    Down::Told { id, answer: Ok(Told::Done) } if identity.controller => {
                        stats.expected_group.fetch_max(id, Ordering::Relaxed);
                        stats.pending_changes.fetch_sub(1, Ordering::Relaxed);
                        stats.changes.fetch_add(1, Ordering::Relaxed);
                    }
                    Down::Presence { machine, presence, rev } => {
                        if state.presences.get(&machine).is_none_or(|(previous, _)| rev > *previous) {
                            state.checked.presence(&machine, presence.online)?;
                            state.presences.insert(machine.clone(), (rev, presence.online));
                            if let Some(view) = &mut state.view && let Some(member) = view.directory.members.iter_mut().find(|member| member.name == machine) { Arc::make_mut(member).online = presence.online; }
                        }
                    }
                    Down::Told { answer: Err(why), .. } => anyhow::bail!("{}: {why}", identity.name),
                    Down::Ack { seq, refused } => {
                        anyhow::ensure!(refused.is_none(), "letter refused: {refused:?}");
                        if state.pending.is_some() && seq >= state.next {
                            stats.letters_held.fetch_add(1, Ordering::Relaxed); state.pending = None;
                            state.held = seq; state.next = seq + 1;
                        }
                    }
                    Down::Deliver { seq, mut message } => {
                        if stats.fault.as_deref() == Some("letter") { message.to = "wrong-recipient".parse().expect("fault name"); }
                        if state.checked.letter(identity, &message)? { stats.letters_received.fetch_add(1, Ordering::Relaxed); }
                        else { stats.wire_replays.fetch_add(1, Ordering::Relaxed); }
                        if let Some((epoch, taken)) = state.taken {
                            anyhow::ensure!(seq <= taken + 1, "{} received hub sequence {seq} after {taken}", identity.name);
                            state.taken = Some((epoch, taken.max(seq)));
                        }
                        dial::send(&mut socket, &Up::Ack { seq }).await.map_err(anyhow::Error::msg)?;
                    }
                    Down::Refused { reason } => anyhow::bail!("{}: {reason}", identity.name),
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(pong_deadline) => anyhow::bail!("{} received no matching Pong within twelve seconds", identity.name),
            _ = tokio::time::sleep_until(heard + Duration::from_secs(30)) => anyhow::bail!("{} heard no hub traffic for 30 seconds", identity.name),
        }
    }
}

pub fn greeting(identity: &Identity, binding: &[u8; 32], position: Letters, address: SocketAddr) -> Up {
    let proof = identity.key.sign(fastctx_net::sign::Domain::Session, binding);
    Up::Hello {
        protocol: wire::GREETING_PROTOCOL,
        version: env!("CARGO_PKG_VERSION").into(),
        intent: Intent::Session {
            asset_sync: None,
            key: identity.key.public(),
            proof,
            letters: Letters { epoch: position.epoch, taken: position.taken },
            through: None,
            direct: false,
            interfaces: Vec::new(),
            dialed: Some(address),
            any_names: true,
            renames: true,
            afoot: !identity.controller,
            forgets: true,
            roles: true,
            versions: false,
            version_parts: true,
            due: true,
            project_deletes: true,
            project_entry: true,
            conversation_deletions: true,
            forgets_accounts: false,
            profiles: true,
            machine_updates: true,
            history_switch: true,
            account_records: false,
            record_keys: false,
            skill_files: false,
            conversation_plans: false,
            scratch_locations: true,
            left: None,
            cancels: true,
            presence: true,
            parts: true,
            call_details: true,
            view_deltas: !identity.controller,
            replicated_records: true,
        },
    }
}
