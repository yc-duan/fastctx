//! What a hub put back from an older copy is handed back while its assets
//! are sealed for the recovery (design §28.38). The seal writes nothing new
//! from the moment it is on, so where the hub holds a stream it orders is
//! where the copy left it; a device that holds frames the hub signed past
//! that point, in the same epoch, gives them back one at a time with the
//! objects they refer to. Each one checked — under a key of the hub's
//! keyring, of the stream's epoch here, following the hub's tail or a frame
//! handed back already — is kept as evidence of its scope's holding period
//! and noted in `recovery_frames`. As the hub reopens it takes back the
//! longest chain of each stream (`take_back`), and says what it did in
//! `recovery_results`. A frame forking before the tail, of an epoch or a
//! scope the hub does not hold, is not kept; why is counted in
//! `recovery_refusals`, and the device keeps its frames as evidence.

use super::scopes::{self, Scope};
use super::{Assets, Owner};
use fastctx_content::v2::{
    Library, RawFrame, Received, Standing, Taking, Verifier, hub_orders, read_handed_back,
};
use fastctx_content::ObjectRef;
use fastctx_net::directory_index::Indexed;
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::MachineName;
use fastctx_types::assets::{self as wire, Bytes, Reply};
use std::collections::{BTreeMap, HashMap};

/// The tables of what was handed back and taken back, in the hub's control
/// database. `recovery_frames` lasts until the hub reopens; the results and
/// refusals are kept after, for the report and the status page.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS recovery_frames (
    recovery INTEGER NOT NULL,
    owner TEXT NOT NULL,
    scope TEXT NOT NULL,
    stream TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    first INTEGER NOT NULL,
    count INTEGER NOT NULL,
    previous BLOB NOT NULL,
    chain BLOB NOT NULL,
    bytes INTEGER NOT NULL,
    machine TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (recovery, scope, stream, epoch, first, chain)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS recovery_results (
    recovery INTEGER NOT NULL,
    owner TEXT NOT NULL,
    scope TEXT NOT NULL,
    stream TEXT NOT NULL,
    tail_next INTEGER NOT NULL,
    tail_head BLOB NOT NULL,
    chosen_next INTEGER NOT NULL,
    chosen_head BLOB NOT NULL,
    applied_next INTEGER NOT NULL,
    applied_head BLOB NOT NULL,
    frames INTEGER NOT NULL,
    branches INTEGER NOT NULL,
    repaired INTEGER NOT NULL DEFAULT 0,
    stopped TEXT,
    PRIMARY KEY (recovery, scope, stream)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS recovery_refusals (
    recovery INTEGER NOT NULL,
    scope TEXT NOT NULL,
    stream TEXT NOT NULL,
    why TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (recovery, scope, stream, why)
) WITHOUT ROWID;
";

/// Why a frame handed back is evidence of: what the hub signed past where
/// it was put back.
pub(crate) const WHY: &str = "handed back";

/// The most frames, and bytes, one owner's scopes are handed back in one
/// recovery: past either, a frame is answered not kept.
const OWNER_FRAMES_MOST: i64 = 1 << 16;
const OWNER_BYTES_MOST: i64 = 1 << 30;

/// The reasons a frame handed back is not kept, as they are counted.
const FORKED: &str = "it does not follow the hub's tail or a frame handed back";
const NO_EPOCH: &str = "it is of an epoch the hub's library does not stand in";
const NO_SCOPE: &str = "its scope is not in the hub as it was put back";
const FULL: &str = "its owner handed back as much as one recovery keeps";
const UNSIGNED: &str = "it is not signed by a key of the hub's keyring";
const NOT_HUBS: &str = "its stream is not one the hub orders";
const OBJECTS: &str = "an object it refers to is missing or does not match";

/// Checks a frame under the hub's keyring: its key one the hub holds or
/// held, its signature good in the domain frames are signed in.
pub(crate) struct InRing<'a>(pub &'a super::keys::Keys);

impl Verifier for InRing<'_> {
    fn verify(&self, header: &fastctx_content::v2::Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        if !self.0.holds(&header.key) || header.device != header.key {
            return false;
        }
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key).verify(Domain::SyncSourceBatch, header_bytes, &Signature::from_bytes(signature)).is_ok()
    }
}

/// The library of `scope`'s owner as it is, the scope installed in it, not
/// judged against the hub's record of it: the hub is sealed for its
/// recovery already, and what it serves now only helps it recover.
pub(super) fn library_as_is(assets: &Assets, scope: &Scope) -> Result<Library, String> {
    let library = assets.owners.library(&scope.owner, |_| Ok(None)).map_err(|error| error.to_string())??;
    let key = &scope.descriptor.scope;
    if library.holding(key).map_err(|error| error.to_string())?.is_none() {
        library.install(key, scope.fact.fact.0.clone(), scope.descriptor.seq as i64).map_err(|error| error.to_string())?;
    }
    Ok(library)
}

/// The scope `key`, if `machine` reads it and holds `stream` of it, by the
/// directory as it stands; the gates of an open hub, without what they
/// note.
fn gate(assets: &Assets, directory: &Indexed<'_>, machine: &MachineName, key: &str, stream: &str) -> Result<Option<Scope>, String> {
    let Some(scope) = scopes::scope(assets, key)? else { return Ok(None) };
    if !scopes::readable(directory, machine, &scope) || !scopes::holds(directory, machine, &scope, stream) {
        return Err(format!("this machine does not hold {stream} of the scope {key}"));
    }
    Ok(Some(scope))
}

fn chain_value(bytes: &Bytes, what: &str) -> Result<[u8; 32], String> {
    <[u8; 32]>::try_from(bytes.0.as_slice()).map_err(|_| format!("{what} is not 32 bytes"))
}

/// Answers where the hub, sealed for its recovery, holds each of the
/// streams `streams` names of the scope `key`, as an open hub answers a
/// device's cursors: a device finds from it where what it holds goes past.
pub(super) fn cursors(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    device: &PublicKey,
    key: &str,
    holding: i64,
    streams: Vec<wire::Cursor>,
) -> Result<Reply, String> {
    let mut standing = Vec::with_capacity(streams.len());
    let mut library = None;
    for cursor in streams {
        if !hub_orders(&cursor.stream) {
            return Err(format!("{} is not a stream the hub orders", cursor.stream));
        }
        // A scope the hub put back does not hold — one born since its copy
        // — has nothing of it to hand back: told so, the device goes on with
        // the next (codex-f92d833e #1).
        let Some(scope) = gate(assets, directory, machine, key, &cursor.stream)? else {
            return Ok(Reply::Standing { streams: Vec::new() });
        };
        let library = match &library {
            Some(library) => library,
            None => library.insert(library_as_is(assets, &scope)?),
        };
        let head = chain_value(&cursor.head, "a cursor's chain value")?;
        let said = library.check_cursor(device.as_bytes(), key, &cursor.stream, (holding, cursor.sequence, head)).map_err(|error| error.to_string())?;
        standing.push(match said {
            Standing::Agrees => wire::Standing { stream: cursor.stream, agrees: true, ends: Vec::new() },
            Standing::Differs(ends) => wire::Standing {
                stream: cursor.stream,
                agrees: false,
                ends: ends.into_iter().map(|(end, head)| (end, head.into())).collect(),
            },
        });
    }
    Ok(Reply::Standing { streams: standing })
}

/// How many kinds of refusal — scope, stream and why — one recovery
/// counts at most; past them, refusals already counted still count.
const REFUSALS_MOST: i64 = 4096;

fn refused(assets: &Assets, recovery: u64, scope: &str, stream: &str, why: &'static str) -> Result<Reply, String> {
    let (scope, stream) = (scope.to_string(), stream.to_string());
    assets
        .control
        .write_now(256, move |c| {
            let at = params![recovery as i64, scope, stream, why];
            let counted = c
                .prepare_cached("UPDATE recovery_refusals SET count = count + 1 WHERE recovery = ?1 AND scope = ?2 AND stream = ?3 AND why = ?4")?
                .execute(at)?;
            if counted == 0 {
                let kinds: i64 =
                    c.prepare_cached("SELECT COUNT(*) FROM recovery_refusals WHERE recovery = ?1")?.query_row([recovery as i64], |row| row.get(0))?;
                if kinds < REFUSALS_MOST {
                    c.prepare_cached("INSERT INTO recovery_refusals (recovery, scope, stream, why, count) VALUES (?1, ?2, ?3, ?4, 1)")?
                        .execute(params![recovery as i64, scope, stream, why])?;
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(Reply::HandedBack { kept: false, why: Some(why.to_string()) })
}

/// Takes a frame `machine` hands back of `stream` of the scope `key`, with
/// the bytes of objects its records refer to, for the recovery `recovery`
/// (the count of recoveries the hub reopens with): kept if it checks, else
/// answered not kept with why.
#[allow(clippy::too_many_arguments)]
pub(super) fn hand_back(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    recovery: u64,
    key: String,
    stream: String,
    frame: wire::Frame,
    objects: Vec<(Bytes, Bytes)>,
) -> Result<Reply, String> {
    // Of a stream the hub does not order, or a scope it does not hold, the
    // refusal is not counted: no name a machine makes up is kept
    // (codex-f92d833e #3).
    if !hub_orders(&stream) {
        return Ok(Reply::HandedBack { kept: false, why: Some(NOT_HUBS.to_string()) });
    }
    let Some(scope) = gate(assets, directory, machine, &key, &stream)? else {
        return Ok(Reply::HandedBack { kept: false, why: Some(NO_SCOPE.to_string()) });
    };
    if frame.apart > 0 {
        return Err("a frame handed back carries its body with it".into());
    }
    let raw = RawFrame { header: frame.header.0, signature: frame.signature.0, body: frame.body.0, objects: Vec::new() };
    let Ok(read) = read_handed_back(&key, &raw) else { return refused(assets, recovery, &key, &stream, UNSIGNED) };
    let header = read.header;
    if header.stream != stream {
        return Err("the frame is of another stream".into());
    }
    if !InRing(assets.keys()).verify(&header, &raw.header, &raw.signature) {
        return refused(assets, recovery, &key, &stream, UNSIGNED);
    }
    let library = library_as_is(assets, &scope)?;
    let Some(tail) = library.tail(&key, &stream).map_err(|error| error.to_string())? else {
        return refused(assets, recovery, &key, &stream, NO_SCOPE);
    };
    let of_epoch = match tail.orderer {
        Some((device, signer)) => header.epoch == tail.epoch && header.device == device && header.key == signer,
        None => header.epoch == 0,
    };
    if !of_epoch {
        return refused(assets, recovery, &key, &stream, NO_EPOCH);
    }
    let chain = header.chain();
    let (owner, previous, first, epoch) = (scope.owner.key(), header.previous, header.first, header.epoch);
    let (scope_key, name) = (key.clone(), stream.clone());
    // Kept already, or following what is: a staged frame ends where this one
    // starts, with the chain value it follows.
    let (kept, follows, used) = assets
        .control
        .read_now(move |c| {
            let at = params![recovery as i64, scope_key, name, epoch as i64];
            let kept = c
                .prepare_cached(
                    "SELECT 1 FROM recovery_frames WHERE recovery = ?1 AND scope = ?2 AND stream = ?3 AND epoch = ?4
                     AND first = ?5 AND chain = ?6",
                )?
                .query_row(params![at[0], at[1], at[2], at[3], first as i64, &chain[..]], |_| Ok(()))
                .optional()?
                .is_some();
            let follows = c
                .prepare_cached(
                    "SELECT 1 FROM recovery_frames WHERE recovery = ?1 AND scope = ?2 AND stream = ?3 AND epoch = ?4
                     AND first + count = ?5 AND chain = ?6",
                )?
                .query_row(params![at[0], at[1], at[2], at[3], first as i64, &previous[..]], |_| Ok(()))
                .optional()?
                .is_some();
            let used: (i64, i64) = c
                .prepare_cached("SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM recovery_frames WHERE recovery = ?1 AND owner = ?2")?
                .query_row(params![recovery as i64, owner], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok((kept, follows, used))
        })
        .map_err(|error| error.to_string())?;
    if kept {
        return Ok(Reply::HandedBack { kept: true, why: None });
    }
    let on_tail = first == tail.next && previous == tail.head;
    if !on_tail && !follows {
        return refused(assets, recovery, &key, &stream, FORKED);
    }
    let bytes: usize = raw.header.len() + raw.signature.len() + raw.body.len() + objects.iter().map(|(_, bytes)| bytes.0.len()).sum::<usize>();
    if used.0 >= OWNER_FRAMES_MOST || used.1 + bytes as i64 > OWNER_BYTES_MOST {
        return refused(assets, recovery, &key, &stream, FULL);
    }
    let given: Vec<Vec<u8>> = objects.into_iter().map(|(_, bytes)| bytes.0).collect();
    if library.keep_handed_back(&key, &raw, &given, WHY).map_err(|error| error.to_string())?.is_err() {
        return refused(assets, recovery, &key, &stream, OBJECTS);
    }
    let (owner, at) = (scope.owner.key(), jiff::Timestamp::now().as_millisecond());
    let machine = machine.to_string();
    let count = i64::from(header.count);
    assets
        .control
        .write_now(512, move |c| {
            c.prepare_cached(
                "INSERT OR IGNORE INTO recovery_frames
                 (recovery, owner, scope, stream, epoch, first, count, previous, chain, bytes, machine, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         MAX(?12, (SELECT COALESCE(MAX(at), 0) + 1 FROM recovery_frames WHERE recovery = ?1)))",
            )?
            .execute(params![
                recovery as i64,
                owner,
                key,
                stream,
                epoch as i64,
                first as i64,
                count,
                &previous[..],
                &chain[..],
                bytes as i64,
                machine,
                at
            ])?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(Reply::HandedBack { kept: true, why: None })
}

/// A frame noted as handed back.
#[derive(Clone)]
struct Noted {
    epoch: u64,
    first: u64,
    count: u64,
    previous: [u8; 32],
    chain: [u8; 32],
    at: i64,
}

/// A row of `recovery_frames` as read: owner, scope, stream, epoch, first,
/// count, previous, chain, and when it came.
type FrameRow = (String, String, String, i64, i64, i64, Vec<u8>, Vec<u8>, i64);

/// The frames handed back for `recovery`, by owner, scope and stream.
fn noted_of(assets: &Assets, recovery: u64) -> anyhow::Result<BTreeMap<(String, String, String), Vec<Noted>>> {
    let rows: Vec<FrameRow> = assets.control.read_now(move |c| {
        let mut statement = c.prepare_cached(
            "SELECT owner, scope, stream, epoch, first, count, previous, chain, at FROM recovery_frames WHERE recovery = ?1",
        )?;
        let rows = statement.query_map([recovery as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    })?;
    let mut noted: BTreeMap<(String, String, String), Vec<Noted>> = BTreeMap::new();
    for (owner, scope, stream, epoch, first, count, previous, chain, at) in rows {
        let value = |bytes: Vec<u8>| <[u8; 32]>::try_from(bytes).unwrap_or([0; 32]);
        noted.entry((owner, scope, stream)).or_default().push(Noted {
            epoch: epoch as u64,
            first: first as u64,
            count: count as u64,
            previous: value(previous),
            chain: value(chain),
            at,
        });
    }
    Ok(noted)
}

/// The longest chain of `frames` from the tail (`next`, `head`) in `epoch`:
/// its frames in order. Of two as long, the one whose frames came first.
fn longest(frames: &[Noted], epoch: u64, next: u64, head: [u8; 32]) -> Vec<Noted> {
    // Each frame's farthest end and the frame it goes on to, found from the
    // ends back: a frame is followed only by frames later in sequence, found
    // by where they start and what they follow (codex-f92d833e #10). At a
    // fork, of two that reach as far, the one handed back first.
    let mut order: Vec<&Noted> = frames.iter().filter(|frame| frame.epoch == epoch).collect();
    order.sort_by_key(|frame| std::cmp::Reverse(frame.first));
    let mut following: HashMap<(u64, [u8; 32]), Vec<&Noted>> = HashMap::new();
    for frame in &order {
        following.entry((frame.first, frame.previous)).or_default().push(frame);
    }
    let by_chain: HashMap<[u8; 32], &Noted> = order.iter().map(|frame| (frame.chain, *frame)).collect();
    let pick = |candidates: Option<&Vec<&Noted>>, best: &HashMap<[u8; 32], (u64, Option<[u8; 32]>)>| {
        let mut chosen: Option<(u64, i64, [u8; 32])> = None;
        for candidate in candidates.into_iter().flatten() {
            let reach = best[&candidate.chain].0;
            if chosen.is_none_or(|(most, at, _)| reach > most || (reach == most && candidate.at < at)) {
                chosen = Some((reach, candidate.at, candidate.chain));
            }
        }
        chosen
    };
    let mut best: HashMap<[u8; 32], (u64, Option<[u8; 32]>)> = HashMap::new();
    for frame in &order {
        let end = frame.first + frame.count;
        let chosen = pick(following.get(&(end, frame.chain)), &best);
        best.insert(frame.chain, (chosen.map_or(end, |(reach, ..)| reach), chosen.map(|(.., chain)| chain)));
    }
    let mut chain = Vec::new();
    let mut at = pick(following.get(&(next, head)), &best).map(|(.., chain)| chain);
    while let Some(value) = at {
        let Some(frame) = by_chain.get(&value) else { break };
        chain.push((*frame).clone());
        at = best[&value].1;
    }
    chain
}

/// How many branches of `frames` are left beside the chain `chosen`: the
/// frames not chosen that follow none of the others not chosen.
fn branches(frames: &[Noted], chosen: &[Noted]) -> i64 {
    let chosen: std::collections::HashSet<[u8; 32]> = chosen.iter().map(|frame| frame.chain).collect();
    let left: std::collections::HashSet<[u8; 32]> = frames.iter().filter(|frame| !chosen.contains(&frame.chain)).map(|frame| frame.chain).collect();
    frames.iter().filter(|frame| left.contains(&frame.chain) && !left.contains(&frame.previous)).count() as i64
}

/// Takes back, as the hub sealed for the recovery `recovery` reopens, the
/// longest chain handed back of each stream (design §28.38 four): chosen
/// once and kept in `recovery_results`, then taken frame by frame through
/// the hub's own taking, where it stopped noted as it goes, so a reopening
/// cut short goes on from there. The account records the frames hold are
/// written back after (`handback_repair`). Lines for the report.
pub(super) fn take_back(
    dir: &std::path::Path,
    directory: &fastctx_net::directory::Directory,
    assets: &Assets,
    recovery: u64,
) -> anyhow::Result<Vec<String>> {
    let noted = noted_of(assets, recovery)?;
    let mut lines = Vec::new();
    for ((owner_key, key, stream), frames) in &noted {
        let Some(owner) = Owner::from_key(owner_key) else { continue };
        let Some(scope) = scopes::scope(assets, key).map_err(anyhow::Error::msg)? else { continue };
        if scope.owner != owner {
            continue;
        }
        let library = library_as_is(assets, &scope).map_err(anyhow::Error::msg)?;
        let Some(tail) = library.tail(key, stream)? else { continue };
        let (k, s) = (key.clone(), stream.clone());
        let chosen: Option<(i64, Vec<u8>)> = assets.control.read_now(move |c| {
            Ok(c.prepare_cached("SELECT chosen_next, chosen_head FROM recovery_results WHERE recovery = ?1 AND scope = ?2 AND stream = ?3")?
                .query_row(params![recovery as i64, k, s], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?)
        })?;
        // Chosen once: a reopening cut short walks the same chain again,
        // from its first frame, back from where it was chosen to end.
        let chain = match chosen {
            Some((_, chosen_head)) => {
                let (k, s) = (key.clone(), stream.clone());
                let (tail_next, tail_head): (i64, Vec<u8>) = assets.control.read_now(move |c| {
                    Ok(c.prepare_cached("SELECT tail_next, tail_head FROM recovery_results WHERE recovery = ?1 AND scope = ?2 AND stream = ?3")?
                        .query_row(params![recovery as i64, k, s], |row| Ok((row.get(0)?, row.get(1)?)))?)
                })?;
                let tail_head = <[u8; 32]>::try_from(tail_head).unwrap_or([0; 32]);
                let chosen_head = <[u8; 32]>::try_from(chosen_head).unwrap_or([0; 32]);
                let mut back = Vec::new();
                let mut at = chosen_head;
                while let Some(frame) = frames.iter().find(|frame| frame.chain == at) {
                    back.push(frame.clone());
                    if frame.first == tail_next as u64 && frame.previous == tail_head {
                        break;
                    }
                    at = frame.previous;
                }
                back.reverse();
                back
            }
            None => {
                let chain = longest(frames, tail.epoch, tail.next, tail.head);
                let (end, head) = chain.last().map_or((tail.next, tail.head), |frame| (frame.first + frame.count, frame.chain));
                let branches = branches(frames, &chain);
                let (owner_key, k, s, tail_head) = (owner_key.clone(), key.clone(), stream.clone(), tail.head);
                let tail_next = tail.next;
                assets.control.write_now(512, move |c| {
                    c.prepare_cached(
                        "INSERT OR IGNORE INTO recovery_results
                         (recovery, owner, scope, stream, tail_next, tail_head, chosen_next, chosen_head, applied_next, applied_head, frames, branches)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?5, ?6, 0, ?9)",
                    )?
                    .execute(params![recovery as i64, owner_key, k, s, tail_next as i64, &tail_head[..], end as i64, &head[..], branches])?;
                    Ok(())
                })?;
                chain
            }
        };
        let (applied, stopped) = apply_chain(assets, &library, &scope, stream, recovery, &chain)?;
        lines.push(match stopped {
            None => format!("{key} {stream}: took back {applied} frame(s) handed back"),
            Some(why) => format!("{key} {stream}: took back {applied} frame(s) handed back, then stopped: {why}"),
        });
    }
    lines.extend(super::handback_repair::repair(dir, directory, assets, recovery)?);
    Ok(lines)
}

/// Takes the frames of `chain` the library does not hold yet, in order;
/// how many it took, and why it stopped if it did before the end. How far
/// the chain stands taken is noted from the library itself, as it ends:
/// a frame taken by a reopening cut short before it noted it counts too.
fn apply_chain(
    assets: &Assets,
    library: &Library,
    scope: &Scope,
    stream: &str,
    recovery: u64,
    chain: &[Noted],
) -> anyhow::Result<(u64, Option<String>)> {
    let key = scope.descriptor.scope.as_str();
    let period = library.holding(key)?.ok_or_else(|| anyhow::anyhow!("the scope {key} is not held"))?;
    let anything = |_: &ObjectRef| true;
    let nothing = |_: &[u8; 32]| false;
    let taking = Taking { period, readable: &anything, proven: &nothing };
    let mut applied = 0u64;
    let mut stopped = None;
    for frame in chain {
        let tail = library.tail(key, stream)?.ok_or_else(|| anyhow::anyhow!("the scope {key} is not held"))?;
        // Taken already, by a reopening cut short.
        if frame.first + frame.count <= tail.next {
            continue;
        }
        if frame.first != tail.next || frame.previous != tail.head {
            stopped = Some("the library no longer ends where the chain goes on".to_string());
            break;
        }
        let Some(raw) = library.evidence_frame(key, stream, (frame.epoch, frame.first), &frame.chain)? else {
            stopped = Some("a frame chosen is no longer in the evidence".to_string());
            break;
        };
        match library.take(key, raw, &InRing(assets.keys()), &taking, super::serve::kinds(), assets.signer())? {
            Received::Applied { .. } => applied += 1,
            other => {
                stopped = Some(format!("the hub did not take a frame handed back: {other:?}"));
                break;
            }
        }
    }
    let tail = library.tail(key, stream)?.ok_or_else(|| anyhow::anyhow!("the scope {key} is not held"))?;
    let held: Vec<&Noted> = chain.iter().take_while(|frame| frame.first + frame.count <= tail.next).collect();
    let (count, end, head) = match held.last() {
        Some(last) => (held.len() as i64, (last.first + last.count) as i64, last.chain),
        None => (0, -1, [0; 32]),
    };
    let (k, s, why) = (key.to_string(), stream.to_string(), stopped.clone());
    assets.control.write_now(512, move |c| {
        if end >= 0 {
            c.prepare_cached(
                "UPDATE recovery_results SET applied_next = ?4, applied_head = ?5, frames = ?6
                 WHERE recovery = ?1 AND scope = ?2 AND stream = ?3",
            )?
            .execute(params![recovery as i64, k, s, end, &head[..], count])?;
        }
        c.prepare_cached("UPDATE recovery_results SET stopped = ?4 WHERE recovery = ?1 AND scope = ?2 AND stream = ?3")?
            .execute(params![recovery as i64, k, s, why])?;
        Ok(())
    })?;
    Ok((applied, stopped))
}

/// What the report says of what was handed back for the recovery
/// `recovery` while sealed: frames kept by stream, and what was not kept.
pub(super) fn report(assets: &Assets, recovery: u64) -> anyhow::Result<Vec<String>> {
    type Kept = Vec<(String, String, i64, i64)>;
    type Refused = Vec<(String, String, String, i64)>;
    let (kept, refused): (Kept, Refused) = assets.control.read_now(move |c| {
        let kept = c
            .prepare_cached(
                "SELECT scope, stream, COUNT(*), COUNT(DISTINCT machine) FROM recovery_frames WHERE recovery = ?1
                 GROUP BY scope, stream ORDER BY scope, stream",
            )?
            .query_map([recovery as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<_, _>>()?;
        let refused = c
            .prepare_cached("SELECT scope, stream, why, count FROM recovery_refusals WHERE recovery = ?1 ORDER BY scope, stream, why")?
            .query_map([recovery as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<_, _>>()?;
        Ok((kept, refused))
    })?;
    let mut lines = Vec::new();
    for (scope, stream, frames, machines) in kept {
        lines.push(format!("handed back: {frames} frame(s) of {stream} in {scope}, from {machines} machine(s); the longest chain is taken back as the hub reopens"));
    }
    for (scope, stream, why, count) in refused {
        lines.push(format!("not kept: {count} frame(s) of {stream} in {scope} handed back: {why}"));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::{Noted, longest};

    fn frame(first: u64, count: u64, previous: u8, chain: u8, at: i64) -> Noted {
        Noted { epoch: 0, first, count, previous: [previous; 32], chain: [chain; 32], at }
    }

    #[test]
    fn the_longest_chain_from_the_tail_is_chosen() {
        // Two branches from the tail (5, [1]): one two frames long, one
        // three; a frame off the tail is never chosen.
        let frames = vec![
            frame(5, 1, 1, 2, 10),
            frame(6, 1, 2, 3, 11),
            frame(5, 1, 1, 4, 20),
            frame(6, 2, 4, 5, 21),
            frame(9, 1, 5, 6, 22),
            frame(7, 1, 9, 7, 1),
        ];
        let chain: Vec<u8> = longest(&frames, 0, 5, [1; 32]).iter().map(|frame| frame.chain[0]).collect();
        assert_eq!(chain, vec![4, 5]);
        // Beside it, the branch from the tail two frames long and the two
        // frames that go on from nothing kept: three branches, four frames.
        let chosen = longest(&frames, 0, 5, [1; 32]);
        assert_eq!(super::branches(&frames, &chosen), 3);
    }

    #[test]
    fn of_two_as_long_the_first_handed_back_is_chosen() {
        let frames = vec![frame(5, 2, 1, 2, 30), frame(5, 2, 1, 3, 10)];
        let chain: Vec<u8> = longest(&frames, 0, 5, [1; 32]).iter().map(|frame| frame.chain[0]).collect();
        assert_eq!(chain, vec![3]);
        assert!(longest(&frames, 1, 5, [1; 32]).is_empty());
        assert!(longest(&frames, 0, 4, [1; 32]).is_empty());
    }
}
