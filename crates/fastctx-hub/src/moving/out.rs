//! The old hub's side of a move, until it records the switch.
//!
//! Once the new place made its directory, the old hub dials it at its new
//! address, until it reaches it: before the hub's service runs there,
//! failing is what is expected, and the move says it waits; after, the
//! failure is said. Reached, it copies its records across while it still
//! serves, then stops taking anything (`Moving::freeze`), copies what
//! changed meanwhile, and has the new place take over when it holds exactly
//! what is here. Recording the switch is the last step: before it, any
//! failure thaws the hub, which serves on and tries again; after it, the
//! hub only forwards (`super::old`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use fastctx_net::dial::{self, HubSocket};
use fastctx_net::moving::HandoverRole;
use fastctx_net::sign::{Digest, Domain};
use fastctx_net::wire::{self, Intent, Up};
use fastctx_types::hub_move::{MoveStage, OldPlace};
use jiff::Timestamp;

use super::files::{self, Entry, Sum};
use super::frames::{self, Record, ToNew, ToOld};
use super::{Moving, Outgoing};
use crate::Hub;

/// The largest message on a handover connection: a piece of a file, or a
/// part of a listing, with room to spare.
pub(crate) const HANDOVER_MESSAGE: usize = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;

/// Drives the move under way, and tells the new places of moves called off,
/// unless a task does already; that one is woken to try again now.
pub(crate) fn drive(hub: &Arc<Hub>) {
    hub.moving.poke.notify_one();
    if hub.moving.driving.swap(true, Ordering::SeqCst) {
        return;
    }
    let hub = hub.clone();
    tokio::spawn(async move {
        loop {
            run(&hub).await;
            hub.moving.driving.store(false, Ordering::SeqCst);
            // Work that came while the task was ending would wait for no one.
            if !driven(&hub) || hub.moving.driving.swap(true, Ordering::SeqCst) {
                return;
            }
        }
    });
}

/// Whether there is anything to drive: a move under way, or a new place of
/// a move called off to tell.
pub(crate) fn driven(hub: &Hub) -> bool {
    let store = hub.store();
    store.outgoing.as_ref().is_some_and(Outgoing::pending) || !store.called_off.is_empty()
}

/// How one try ended.
enum Tried {
    Switched,
    /// The move was called off, or another took its place, while it was
    /// tried.
    Gone,
    /// It failed, and why; `quiet` while the failure is what waiting for
    /// the new place's service looks like.
    Failed {
        reason: String,
        quiet: bool,
    },
}

async fn run(hub: &Arc<Hub>) {
    let mut failures = 0;
    // What was last said of each move's new place, so a place that stays
    // away is said once, and again when that changes.
    let mut said: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let poked = hub.moving.poke.notified();
        let untold = hub.store().called_off.clone();
        for record in &untold {
            match tell_called_off(hub, record).await {
                Ok(()) => {
                    said.remove(&record.id);
                }
                Err(error) => {
                    if said.get(&record.id) != Some(&error) {
                        hub.log.line(format_args!(
                            "the new place of the move {}, called off, is still to give the hub up: {error}",
                            record.id
                        ));
                        said.insert(record.id.clone(), error);
                    }
                }
            }
        }
        let record = hub.store().outgoing.clone().filter(Outgoing::pending);
        let still_untold = !hub.store().called_off.is_empty();
        let tried = match record {
            None if !still_untold => return,
            None => None,
            // The new place has not made its directory yet; it says when.
            Some(record) if record.stage == MoveStage::Preparing => {
                if !still_untold {
                    poked.await;
                    continue;
                }
                None
            }
            Some(record) => Some((handover(hub, &record).await, record)),
        };
        match tried {
            Some((Tried::Switched, _)) => return,
            Some((Tried::Gone, _)) => {
                failures = 0;
                continue;
            }
            Some((Tried::Failed { reason, quiet }, record)) => {
                // A new place that stays away is said once, and again when that changes.
                if said.get(&record.id) != Some(&reason) {
                    hub.log.line(format_args!("the move {} to {}: {reason}", record.id, record.to));
                    said.insert(record.id.clone(), reason.clone());
                }
                noted(hub, &record.id, if quiet { None } else { Some(reason) });
            }
            None => {}
        }
        failures += 1;
        tokio::select! {
            () = tokio::time::sleep(fastctx_net::again::backoff(failures)) => {}
            () = poked => failures = 0,
        }
    }
}

/// Records how the move stands after a try, telling those who watch it.
fn noted(hub: &Hub, id: &str, error: Option<String>) {
    let changed = {
        let mut store = hub.store();
        let Some(outgoing) = store.outgoing.as_mut().filter(|outgoing| outgoing.id == id) else { return };
        if outgoing.error == error {
            false
        } else {
            outgoing.error = error;
            if let Err(error) = store.save_outgoing() {
                hub.log.line(format_args!("recording the move: {error}"));
            }
            true
        }
    };
    if changed {
        hub.changed();
    }
}

/// Moves the record on to `stage`, as the move gets there.
fn reached(hub: &Hub, id: &str, stage: MoveStage) {
    {
        let mut store = hub.store();
        let Some(outgoing) = store.outgoing.as_mut().filter(|outgoing| outgoing.id == id) else { return };
        outgoing.stage = stage;
        outgoing.error = None;
        if stage >= MoveStage::Copying {
            outgoing.waiting = None;
        }
        if let Err(error) = store.save_outgoing() {
            hub.log.line(format_args!("recording the move: {error}"));
        }
    }
    hub.changed();
}

fn called_off(hub: &Hub, id: &str) -> bool {
    hub.store().outgoing.as_ref().is_none_or(|outgoing| outgoing.id != id || outgoing.stage == MoveStage::Cancelled)
}

/// Dials the new place for `role`, proving this hub holds the move's key.
pub(crate) async fn dial_new(
    record: &Outgoing,
    pin: fastctx_net::ids::CertPin,
    role: HandoverRole,
) -> Result<HubSocket, String> {
    let dialed = dial::dial(&record.addresses, Some(pin), wire::CONTROL_PATH, HANDOVER_MESSAGE).await?;
    let mut socket = dialed.socket;
    let proof = record.key().sign(Domain::Handover, &dialed.binding);
    let hello = Up::Hello {
        protocol: wire::GREETING_PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Handover { id: record.id.clone(), role, proof },
    };
    dial::send(&mut socket, &hello).await?;
    Ok(socket)
}

async fn handover(hub: &Arc<Hub>, record: &Outgoing) -> Tried {
    if called_off(hub, &record.id) {
        return Tried::Gone;
    }
    let quiet = record.stage == MoveStage::Service && !record.installed;
    let mut socket = match dial_new(record, hub.pin, HandoverRole::Sync).await {
        Ok(socket) => socket,
        Err(error) => {
            return Tried::Failed {
                reason: format!("the hub cannot reach the new place at {}: {error}", list(&record.addresses)),
                quiet,
            };
        }
    };
    match copy(hub, record, &mut socket).await {
        Ok(tried) => tried,
        Err(reason) => {
            if hub.moving.frozen() {
                hub.moving.thaw();
                hub.log.line(format_args!("the hub serves on: the switch did not finish"));
                reached(hub, &record.id, MoveStage::Copying);
            }
            Tried::Failed { reason, quiet: false }
        }
    }
}

async fn copy(hub: &Arc<Hub>, record: &Outgoing, socket: &mut HubSocket) -> Result<Tried, String> {
    let dir = hub.store().dir().to_path_buf();
    let theirs = holds(socket).await?;
    reached(hub, &record.id, MoveStage::Copying);
    hub.log.line(format_args!("the move {}: copying the records to {}", record.id, record.to));
    let mine = listing(socket, &dir).await?;
    send_difference(socket, &dir, &mine, &theirs).await?;
    frames::send(socket, &ToNew::Check).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Checked { .. } => {}
        other => return Err(format!("the new place answered {other:?}")),
    }
    if called_off(hub, &record.id) {
        frames::send(socket, &ToNew::Cancel).await?;
        match frames::frame::<_, ToOld>(socket).await? {
            ToOld::Cancelled => given_up(hub, &record.id),
            other => return Err(format!("the new place answered {other:?}")),
        }
        return Ok(Tried::Gone);
    }

    reached(hub, &record.id, MoveStage::Switching);
    hub.log.line(format_args!("the move {}: the hub stops taking anything and hands over", record.id));
    frames::working::<_, ToNew, _>(socket, Moving::freeze(hub)).await??;
    let theirs = holds(socket).await?;
    let mine = listing(socket, &dir).await?;
    send_difference(socket, &dir, &mine, &theirs).await?;
    let digest = files::digest(&mine);
    frames::send(socket, &ToNew::Check).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Checked { digest: held } if held == digest => {}
        ToOld::Checked { .. } => return Err("the new place does not hold what was sent".to_string()),
        other => return Err(format!("the new place answered {other:?}")),
    }
    let self_registration = hub.store().hub.self_registration;
    let commit = Record { by: record.by.clone(), started: record.started };
    frames::send(socket, &ToNew::Commit { digest, self_registration, record: Box::new(commit) }).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Committed => {}
        ToOld::Refused { reason } => return Err(format!("the new place did not take over: {reason}")),
        other => return Err(format!("the new place answered {other:?}")),
    }

    // The switch, recorded: from here this hub only forwards.
    {
        let mut store = hub.store();
        let outgoing = store.outgoing.as_mut().filter(|outgoing| outgoing.id == record.id).ok_or("the move is gone")?;
        outgoing.switched = Some(Timestamp::now());
        outgoing.stage = MoveStage::Following;
        outgoing.old = Some(OldPlace::Forwarding);
        outgoing.error = None;
        store.save_outgoing().map_err(|error| format!("recording the switch: {error}"))?;
    }
    hub.log.line(format_args!("the move {}: {} took over; this place forwards to it", record.id, record.to));
    let confirmed = async {
        frames::send(socket, &ToNew::Confirm).await?;
        match frames::frame::<_, ToOld>(socket).await? {
            ToOld::Confirmed => Ok(()),
            other => Err(format!("the new place answered {other:?}")),
        }
    };
    if let Err(error) = confirmed.await {
        // The old place confirms it when it connects.
        hub.log.line(format_args!("confirming the switch: {error}"));
    }
    if let Some(switched) = hub.moving.switched.lock().expect("never poisoned").take() {
        let _ = switched.send(());
    }
    Ok(Tried::Switched)
}

/// What the new place holds.
async fn holds(socket: &mut HubSocket) -> Result<Vec<Entry>, String> {
    frames::send(socket, &ToNew::Holds { sqlite: true }).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Holds { sqlite: true, .. } => {}
        ToOld::Holds { sqlite: false, .. } => {
            return Err("update the receiving hub process to 1.0.1-beta.12 or later before moving: it cannot read SQLite hub records".into());
        }
        ToOld::Refused { reason } => return Err(reason),
        other => return Err(format!("the new place answered {other:?}")),
    }
    let mut entries = Vec::new();
    loop {
        match frames::frame::<_, ToOld>(socket).await? {
            ToOld::Files { entries: part } => entries.extend(part),
            ToOld::End => return Ok(entries),
            other => return Err(format!("the new place answered {other:?} in its listing")),
        }
    }
}

/// What is here, listed while the new place is told this end is working.
async fn listing(socket: &mut HubSocket, dir: &std::path::Path) -> Result<Vec<Entry>, String> {
    let dir = dir.to_path_buf();
    let listed = tokio::task::spawn_blocking(move || files::snapshot(&dir));
    frames::working::<_, ToNew, _>(socket, listed)
        .await?
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("listing the records: {error}"))
}

/// Sends what makes `theirs` into `mine`: first the removals of what is
/// not here, or is here as a directory where it is a file there or the
/// other way round, deepest first; then the directories and files that
/// differ.
async fn send_difference(
    socket: &mut HubSocket,
    dir: &std::path::Path,
    mine: &[Entry],
    theirs: &[Entry],
) -> Result<(), String> {
    let kept: BTreeMap<&str, bool> = mine.iter().map(|entry| (entry.path.as_str(), entry.file.is_some())).collect();
    for entry in theirs.iter().rev() {
        if kept.get(entry.path.as_str()) != Some(&entry.file.is_some()) {
            frames::send(socket, &ToNew::Remove { path: entry.path.clone() }).await?;
        }
    }
    let held: BTreeMap<&str, &Option<Sum>> = theirs.iter().map(|entry| (entry.path.as_str(), &entry.file)).collect();
    for entry in mine {
        if held.get(entry.path.as_str()) == Some(&&entry.file) {
            continue;
        }
        match &entry.file {
            None => frames::send(socket, &ToNew::Dir { path: entry.path.clone() }).await?,
            Some(_) => send_file(socket, dir, &entry.path).await?,
        }
    }
    Ok(())
}

/// Sends the file at `path` as it is now, a piece at a time: what was
/// listed may have been replaced since, while the hub still serves.
async fn send_file(socket: &mut HubSocket, dir: &std::path::Path, path: &str) -> Result<(), String> {
    use sha2::Digest as _;
    use tokio::io::AsyncReadExt as _;
    let place = if path == crate::store::shelf::FILE { super::snapshot::path(dir) } else { files::place(dir, path)? };
    let mut file = match tokio::fs::File::open(&place).await {
        Ok(file) => file,
        // Gone since it was listed; the next listing says so.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("reading {path}: {error}")),
    };
    let bytes = file.metadata().await.map_err(|error| format!("reading {path}: {error}"))?.len();
    frames::send(socket, &ToNew::Put { path: path.to_string(), bytes }).await?;
    let mut hasher = sha2::Sha256::new();
    let mut left = bytes;
    while left > 0 {
        let mut piece = vec![0; usize::try_from(left).unwrap_or(usize::MAX).min(frames::CHUNK)];
        file.read_exact(&mut piece).await.map_err(|error| format!("reading {path}: {error}"))?;
        hasher.update(&piece);
        left -= piece.len() as u64;
        frames::send_bytes(socket, piece).await?;
    }
    frames::send(socket, &ToNew::Sum { sha256: Digest(hasher.finalize().into()) }).await
}

/// Tells the new place of a move called off to give the hub up. It did
/// when it says so, or when it stands by for no such move any more.
async fn tell_called_off(hub: &Hub, record: &Outgoing) -> Result<(), String> {
    let told = async {
        let mut socket = dial_new(record, hub.pin, HandoverRole::Sync).await?;
        frames::send(&mut socket, &ToNew::Cancel).await?;
        match frames::frame::<_, ToOld>(&mut socket).await? {
            ToOld::Cancelled => Ok(()),
            other => Err(format!("the new place answered {other:?}")),
        }
    };
    match told.await {
        Err(error) if !error.contains(super::NOT_ARRIVING) => Err(error),
        _ => {
            given_up(hub, &record.id);
            Ok(())
        }
    }
}

/// The new place of the move `id`, called off, holds nothing of it any more.
fn given_up(hub: &Hub, id: &str) {
    hub.log.line(format_args!("the new place gave the move {id} up"));
    {
        let mut store = hub.store();
        store.called_off.retain(|untold| untold.id != id);
        if let Err(error) = store.save_called_off() {
            hub.log.line(format_args!("recording the move: {error}"));
        }
    }
    hub.changed();
}

fn list(addresses: &[std::net::SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
