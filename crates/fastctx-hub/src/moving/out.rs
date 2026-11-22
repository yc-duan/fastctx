//! The old hub's side of a move, until it records the switch.
//!
//! Once the new place made its directory, the old hub dials it at its new
//! address, until it reaches it: before the hub's service runs there,
//! failing is what is expected, and the move says it waits; after, the
//! failure is said. Reached, it copies its core across while it still
//! serves; then stops taking anything (`Moving::freeze`), copies what of
//! the core changed meanwhile, and has the new place take over when it
//! holds exactly the core that is here. Nothing of the owner libraries goes
//! before that: they follow once this place serves no more
//! (`super::libraries`). Recording the switch is the last step: before it,
//! any failure thaws the hub, which serves on; after it, the hub only
//! forwards (`super::old`). A try that failed before the hub stopped is
//! made again by itself; one that failed after it had stopped — or that the
//! hub's own stop cut short while it was stopped — is made again only when
//! the super administrator says so (`requests::retry`): each such try
//! takes the hub from every member for as long as it lasts, and tried
//! again by itself a switch that could not finish took it from them over
//! and over.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use fastctx_net::dial::{self, HubSocket};
use fastctx_net::moving::HandoverRole;
use fastctx_net::sign::{Digest, Domain};
use fastctx_net::wire::{self, Intent, Up};
use fastctx_types::hub_move::{MoveStage, OldPlace};
use jiff::Timestamp;

use super::files::{self, Entry, Part, Sum};
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
    fastctx_platform::task::spawn(async move {
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
    /// the new place's service looks like; `stopped`, after the hub had
    /// stopped taking anything.
    Failed {
        reason: String,
        quiet: bool,
        stopped: bool,
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
            // A switch that did not finish waits for the super
            // administrator, here or after the hub started again.
            Some(record) if record.stage == MoveStage::Preparing || record.stopped_short => {
                if !still_untold {
                    poked.await;
                    continue;
                }
                None
            }
            // Recorded as switching with no try under way: the hub was
            // stopped, or its machine was, while it had stopped taking
            // anything. That try failed as one that could not finish did.
            Some(record) if record.stage == MoveStage::Switching => {
                hub.log.line(format_args!(
                    "the move {} to {} was cut short while the hub handed over: it waits to be tried again or called off",
                    record.id, record.to
                ));
                reached(hub, &record.id, MoveStage::Copying).await;
                stopped_short(hub, &record.id).await;
                noted(hub, &record.id, Some("the hub was stopped while it handed over".to_string())).await;
                // Where that could not be kept — a disk full — it is found
                // so again: after a while, as anything that failed is.
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
            Some((Tried::Failed { reason, quiet, stopped }, record)) => {
                // A new place that stays away is said once, and again when that changes.
                if said.get(&record.id) != Some(&reason) {
                    hub.log.line(format_args!("the move {} to {}: {reason}", record.id, record.to));
                    said.insert(record.id.clone(), reason.clone());
                }
                noted(hub, &record.id, if quiet { None } else { Some(reason) }).await;
                if stopped {
                    // Not again by itself: the record says so, and the next
                    // round waits for the super administrator's word.
                    hub.log.line(format_args!(
                        "the move {} to {} waits to be tried again or called off",
                        record.id, record.to
                    ));
                    said.remove(&record.id);
                    failures = 0;
                    continue;
                }
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
async fn noted(hub: &Hub, id: &str, error: Option<String>) {
    let id = id.to_owned();
    hub.blocking(move |hub| {
        let changed = {
            let mut store = hub.edit_store();
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
    })
    .await;
}

/// Moves the record on to `stage`, as the move gets there.
async fn reached(hub: &Hub, id: &str, stage: MoveStage) {
    let id = id.to_owned();
    hub.blocking(move |hub| {
        {
            let mut store = hub.edit_store();
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
    })
    .await;
}

/// Records that the try just made failed after the hub had stopped.
async fn stopped_short(hub: &Hub, id: &str) {
    let id = id.to_owned();
    hub.blocking(move |hub| {
        let mut store = hub.edit_store();
        let Some(outgoing) = store.outgoing.as_mut().filter(|outgoing| outgoing.id == id) else { return };
        outgoing.stopped_short = true;
        if let Err(error) = store.save_outgoing() {
            hub.log.line(format_args!("recording the move: {error}"));
        }
    })
    .await;
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
                stopped: false,
            };
        }
    };
    let copied = copy(hub, record, &mut socket).await;
    // The closed copies of the databases made for this try are sent or of
    // no more use.
    let dir = hub.store().dir().to_path_buf();
    if let Ok(Err(error)) = fastctx_platform::task::spawn_blocking(move || super::snapshot::clear(&dir, None)).await {
        hub.log.line(format_args!("removing the copies made to hand over: {error}"));
    }
    match copied {
        Ok(tried) => tried,
        Err(reason) => {
            let stopped = hub.moving.frozen();
            // It serves on, and writes of its own accord again.
            hub.moving.thaw();
            if stopped {
                hub.log.line(format_args!("the hub serves on: the switch did not finish"));
                reached(hub, &record.id, MoveStage::Copying).await;
                stopped_short(hub, &record.id).await;
            }
            Tried::Failed { reason, quiet: false, stopped }
        }
    }
}

async fn copy(hub: &Arc<Hub>, record: &Outgoing, socket: &mut HubSocket) -> Result<Tried, String> {
    let dir = hub.store().dir().to_path_buf();
    // While it serves: its core as it is now. What the new place holds
    // beyond it, of an earlier try, goes.
    let theirs = holds(socket, None).await?;
    reached(hub, &record.id, MoveStage::Copying).await;
    hub.log.line(format_args!("the move {}: copying the records to {}", record.id, record.to));
    let mine = listing(socket, &dir, Part::Core).await?;
    let sent = send_difference(socket, &dir, &mine, &theirs, true).await?;
    hub.log.line(format_args!(
        "the move {}: {} bytes went of the {} files the new place lacked, {} bytes long",
        record.id, sent.bytes, sent.files, sent.whole
    ));
    frames::send(socket, &ToNew::Check { part: None }).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Checked { .. } => {}
        other => return Err(format!("the new place answered {other:?}")),
    }
    // What the hub writes of its own accord ends first, while it serves:
    // between the copies taken of its record and of its libraries nothing
    // of its own may be written (`Moving::quieten`).
    frames::working::<_, ToNew, _>(socket, Moving::quieten(hub)).await??;
    if called_off(hub, &record.id) {
        hub.moving.thaw();
        frames::send(socket, &ToNew::Cancel).await?;
        match frames::frame::<_, ToOld>(socket).await? {
            ToOld::Cancelled => given_up(hub, &record.id).await,
            other => return Err(format!("the new place answered {other:?}")),
        }
        return Ok(Tried::Gone);
    }

    reached(hub, &record.id, MoveStage::Switching).await;
    hub.log.line(format_args!("the move {}: the hub stops taking anything and hands over", record.id));
    frames::working::<_, ToNew, _>(socket, Moving::freeze(hub)).await??;
    // Asked again with the hub frozen: an assistance ended for good while
    // the records were copied is handed over with them.
    crate::purges::takes_over(hub, &record.to)?;
    // The hub stopped: its core alone, each database a closed copy made now,
    // of which what changed since the copy sent while it served is sent.
    let theirs = holds(socket, Some(Part::Core)).await?;
    let mine = listing(socket, &dir, Part::Core).await?;
    let sent = send_difference(socket, &dir, &mine, &theirs, true).await?;
    hub.log.line(format_args!(
        "the move {}: with the hub stopped, {} bytes went of the {} files that had changed, {} bytes long",
        record.id, sent.bytes, sent.files, sent.whole
    ));
    let digest = files::digest(&mine);
    frames::send(socket, &ToNew::Check { part: Some(Part::Core) }).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Checked { digest: held } if held == digest => {}
        ToOld::Checked { .. } => return Err("the new place does not hold what was sent".to_string()),
        other => return Err(format!("the new place answered {other:?}")),
    }
    // The owner libraries stay here until this place serves no more.
    let libraries = {
        let dir = dir.clone();
        fastctx_platform::task::spawn_blocking(move || files::libraries(&dir))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("listing the owner libraries: {error}"))?
    };
    let self_registration = hub.store().hub.self_registration;
    let commit = Record { by: record.by.clone(), started: record.started };
    frames::send(socket, &ToNew::Commit { digest, self_registration, record: Box::new(commit), libraries }).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Committed => {}
        ToOld::Refused { reason } => return Err(format!("the new place did not take over: {reason}")),
        other => return Err(format!("the new place answered {other:?}")),
    }

    // The switch, recorded: from here this hub only forwards.
    let id = record.id.clone();
    hub.blocking(move |hub| -> Result<(), String> {
        let mut store = hub.edit_store();
        let outgoing = store.outgoing.as_mut().filter(|outgoing| outgoing.id == id).ok_or("the move is gone")?;
        outgoing.switched = Some(Timestamp::now());
        outgoing.stage = MoveStage::Following;
        outgoing.old = Some(OldPlace::Forwarding);
        outgoing.error = None;
        store.save_outgoing().map_err(|error| format!("recording the switch: {error}"))?;
        Ok(())
    })
    .await?;
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

/// What the new place holds of `part`, or of everything.
pub(super) async fn holds(socket: &mut HubSocket, part: Option<Part>) -> Result<Vec<Entry>, String> {
    frames::send(socket, &ToNew::Holds { sqlite: true, apart: true, part }).await?;
    match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Holds { sqlite: true, apart: true, .. } => {}
        ToOld::Holds { sqlite: true, apart: false, .. } => {
            return Err(format!(
                "update the receiving hub process to {} or later before moving: it would be stopped for every owner library, not for the hub's core alone",
                env!("CARGO_PKG_VERSION")
            ));
        }
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

/// What is here of `part`, listed while the new place is told this end is
/// working.
pub(super) async fn listing(socket: &mut HubSocket, dir: &std::path::Path, part: Part) -> Result<Vec<Entry>, String> {
    let dir = dir.to_path_buf();
    let listed = fastctx_platform::task::spawn_blocking(move || files::snapshot(&dir, &part));
    frames::working::<_, ToNew, _>(socket, listed)
        .await?
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("listing the records: {error}"))
}

/// Sends what makes `theirs` into `mine`: first the removals of what is
/// not here, or is here as a directory where it is a file there or the
/// other way round, deepest first; then the directories and files that
/// differ. `patch`, a database the other end holds another copy of goes as
/// what of it changed (`patch_file`): the hub's own, copied page for page,
/// differ between two copies by the pages written meanwhile, and the hub is
/// stopped for as long as the second takes to go.
pub(super) async fn send_difference(
    socket: &mut HubSocket,
    dir: &std::path::Path,
    mine: &[Entry],
    theirs: &[Entry],
    patch: bool,
) -> Result<Sent, String> {
    let mut sent = Sent::default();
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
        let (bytes, whole) = match &entry.file {
            None => {
                frames::send(socket, &ToNew::Dir { path: entry.path.clone() }).await?;
                continue;
            }
            Some(_) if patch && files::database(&entry.path) && matches!(held.get(entry.path.as_str()), Some(Some(_))) => {
                patch_file(socket, dir, &entry.path).await?
            }
            Some(_) => {
                let bytes = send_file(socket, dir, &entry.path).await?;
                (bytes, bytes)
            }
        };
        sent.files += 1;
        sent.bytes += bytes;
        sent.whole += whole;
    }
    Ok(sent)
}

/// What sending a difference came to: how many files went, how many bytes
/// of them were sent, and how long those files are.
#[derive(Default)]
pub(super) struct Sent {
    pub files: usize,
    pub bytes: u64,
    pub whole: u64,
}

/// Where the bytes of the record at `path` are read from: a database's from
/// the closed copy made of it as it was listed.
fn source(dir: &std::path::Path, path: &str) -> Result<std::path::PathBuf, String> {
    let copy = super::snapshot::path(dir, path);
    if files::database(path) && fastctx_platform::disk::is_there(&copy) { Ok(copy) } else { files::place(dir, path) }
}

/// What a file is against the one the other end holds: how long, its
/// SHA-256, and the runs of its pieces that are not as the other's are.
type Compared = (u64, [u8; 32], Vec<(u64, u64)>);

/// Reads the file at `place` a piece at a time against `theirs`, the
/// digests of the pieces of the other end's.
fn compare(place: &std::path::Path, theirs: &[u8]) -> std::io::Result<Compared> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut file = fastctx_platform::disk::open(place)?;
    let mut whole = sha2::Sha256::new();
    let mut buffer = vec![0; frames::PIECE as usize];
    let (mut bytes, mut at, mut runs) = (0u64, 0u64, Vec::<(u64, u64)>::new());
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            match file.read(&mut buffer[filled..])? {
                0 => break,
                read => filled += read,
            }
        }
        if filled == 0 {
            break;
        }
        whole.update(&buffer[..filled]);
        bytes += filled as u64;
        let held = usize::try_from(at).ok().and_then(|at| theirs.get(at * 32..at * 32 + 32));
        if held != Some(sha2::Sha256::digest(&buffer[..filled]).as_slice()) {
            match runs.last_mut() {
                Some((first, many)) if *first + *many == at => *many += 1,
                _ => runs.push((at, 1)),
            }
        }
        at += 1;
        if filled < buffer.len() {
            break;
        }
    }
    Ok((bytes, whole.finalize().into(), runs))
}

/// Sends the database at `path` as what of it is not in the copy the other
/// end holds: the pieces that differ, the rest taken there from that copy,
/// the whole kept there only when it has this one's digest.
///
/// How many bytes of it were sent, and how long it is.
async fn patch_file(socket: &mut HubSocket, dir: &std::path::Path, path: &str) -> Result<(u64, u64), String> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let place = source(dir, path)?;
    frames::send(socket, &ToNew::Pieces { path: path.to_string(), piece: frames::PIECE }).await?;
    let theirs = match frames::frame::<_, ToOld>(socket).await? {
        ToOld::Pieces { bytes: Some(bytes) } => bytes,
        // No file there after all: whole.
        ToOld::Pieces { bytes: None } => return send_file(socket, dir, path).await.map(|bytes| (bytes, bytes)),
        ToOld::Refused { reason } => return Err(reason),
        other => return Err(format!("the new place answered {other:?}")),
    };
    let wanted = usize::try_from(theirs.div_ceil(u64::from(frames::PIECE)))
        .ok()
        .and_then(|pieces| pieces.checked_mul(32))
        .ok_or_else(|| format!("{path} is too long there to be told in pieces"))?;
    let mut digests = Vec::with_capacity(wanted);
    while digests.len() < wanted {
        match frames::recv::<_, ToOld>(socket, true).await? {
            frames::Got::Bytes(part) => digests.extend_from_slice(&part),
            frames::Got::Frame(_) => return Err(format!("the new place said other than the pieces of {path}")),
        }
    }
    if digests.len() != wanted {
        return Err(format!("the new place told the pieces of {path} amiss"));
    }
    let compared = {
        let place = place.clone();
        fastctx_platform::task::spawn_blocking(move || compare(&place, &digests))
    };
    let (bytes, sha256, changed) = frames::working::<_, ToNew, _>(socket, compared)
        .await?
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("reading {path}: {error}"))?;
    frames::send(socket, &ToNew::Patch { path: path.to_string(), bytes, piece: frames::PIECE, changed: changed.clone() })
        .await?;
    let mut file = tokio::fs::File::open(&place).await.map_err(|error| format!("reading {path}: {error}"))?;
    let piece = u64::from(frames::PIECE);
    let mut went = 0;
    for (first, many) in changed {
        file.seek(std::io::SeekFrom::Start(first * piece)).await.map_err(|error| format!("reading {path}: {error}"))?;
        let mut left = ((first + many) * piece).min(bytes) - first * piece;
        while left > 0 {
            let mut part = vec![0; usize::try_from(left).unwrap_or(usize::MAX).min(frames::CHUNK)];
            file.read_exact(&mut part).await.map_err(|error| format!("reading {path}: {error}"))?;
            left -= part.len() as u64;
            went += part.len() as u64;
            frames::send_bytes(socket, frames::pack(part)).await?;
        }
    }
    frames::send(socket, &ToNew::Sum { sha256: Digest(sha256) }).await?;
    Ok((went, bytes))
}

/// Sends the file at `path` as it is now, a piece at a time: what was
/// listed may have been replaced since, while the hub still serves. How
/// many bytes it is.
async fn send_file(socket: &mut HubSocket, dir: &std::path::Path, path: &str) -> Result<u64, String> {
    use sha2::Digest as _;
    use tokio::io::AsyncReadExt as _;
    let place = source(dir, path)?;
    let mut file = match tokio::fs::File::open(&place).await {
        Ok(file) => file,
        // Gone since it was listed; the next listing says so.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("reading {path}: {error}")),
    };
    let bytes = file.metadata().await.map_err(|error| format!("reading {path}: {error}"))?.len();
    frames::send(socket, &ToNew::Put { path: path.to_string(), bytes, packed: true }).await?;
    let mut hasher = sha2::Sha256::new();
    let mut left = bytes;
    while left > 0 {
        let mut piece = vec![0; usize::try_from(left).unwrap_or(usize::MAX).min(frames::CHUNK)];
        file.read_exact(&mut piece).await.map_err(|error| format!("reading {path}: {error}"))?;
        hasher.update(&piece);
        left -= piece.len() as u64;
        frames::send_bytes(socket, frames::pack(piece)).await?;
    }
    frames::send(socket, &ToNew::Sum { sha256: Digest(hasher.finalize().into()) }).await?;
    Ok(bytes)
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
            given_up(hub, &record.id).await;
            Ok(())
        }
    }
}

/// The new place of the move `id`, called off, holds nothing of it any more.
async fn given_up(hub: &Hub, id: &str) {
    let id = id.to_owned();
    hub.blocking(move |hub| {
        hub.log.line(format_args!("the new place gave the move {id} up"));
        {
            let mut store = hub.edit_store();
            store.called_off.retain(|untold| untold.id != id);
            if let Err(error) = store.save_called_off() {
                hub.log.line(format_args!("recording the move: {error}"));
            }
        }
        hub.changed();
    })
    .await;
}

fn list(addresses: &[std::net::SocketAddr]) -> String {
    addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}
