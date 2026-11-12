//! What the hub answers a device's asset requests (design §6.4–§6.6, §7,
//! §8, §17). Every answer goes through the read gate first: the scope is
//! described, and one of the accounts behind the request reads it in the
//! directory as it stands; a stream is given only to a device its class
//! lets hold it. A hub that found it was put back answers nothing of
//! assets but why.

use super::scopes::{self, Scope};
use super::{Assets, chain};
use fastctx_content::v2::{
    Authorizer, Cutoff, Entry, EpochKind, FrameKey, Head, Header, KindRecord, RawFrame, Received, Standing, Successor, Taking,
    Verifier, applied_statement, parked_statement,
};
use fastctx_content::ObjectRef;
use fastctx_net::directory_index::Indexed;
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::MachineName;
use fastctx_types::assets::{self as wire, Bytes, Reply, Request};
use std::sync::Arc;

/// Frames given in one answer at most.
const FRAMES_AT_ONCE: u32 = 64;
/// Bytes of frames given in one answer at most.
const FRAME_BYTES_AT_ONCE: usize = 4 << 20;

/// Checks a frame's signature against its ordering device's key, under the
/// domain the device signs frames in.
struct DeviceKeys;

impl Verifier for DeviceKeys {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key).verify(Domain::SyncSourceBatch, header_bytes, &Signature::from_bytes(signature)).is_ok()
    }
}

/// The kinds' own rules come with the kinds (design §11, 3-4); until then a
/// record of a scope its writer reads is let through.
struct Readers;

impl Authorizer for Readers {
    fn authorize(&self, _: &Header, _: &KindRecord, _: Option<&Entry>) -> Result<(), String> {
        Ok(())
    }
}

fn array(bytes: &Bytes, what: &str) -> Result<[u8; 32], String> {
    bytes.array().ok_or_else(|| format!("{what} is 32 bytes"))
}

fn object(object: &ObjectRef) -> wire::ObjectRef {
    wire::ObjectRef { hash: object.hash.into(), size: object.size }
}

fn received(received: Received) -> wire::Received {
    match received {
        Received::Applied { change, prefix, signature, drained } => wire::Received::Applied {
            change,
            prefix: prefix.into(),
            word: signature.into(),
            applied: drained.applied,
            through: drained.through,
            refused: drained.refused.map(|refused| wire::Refused {
                header: refused.header.into(),
                why: refused.why,
                word: refused.signature.into(),
            }),
        },
        Received::Parked { sequence, accumulated, signature } => {
            wire::Received::Parked { sequence, accumulated: accumulated.into(), word: signature.into() }
        }
        Received::Rejected { why, from, signature } => wire::Received::Rejected { why, from, word: signature.into() },
        Received::Gap { from, to } => wire::Received::Gap { from, to },
        Received::Missing(objects) => wire::Received::Missing { objects: objects.iter().map(object).collect() },
        Received::Prove(objects) => wire::Received::Prove { objects: objects.iter().map(object).collect() },
    }
}

/// The scope `key`, through the read gate; the machine is told from now
/// on when a stream of it moves.
fn gated(assets: &Assets, directory: &Indexed<'_>, machine: &MachineName, key: &str) -> Result<Scope, String> {
    let scope = scopes::scope(assets, key)?.ok_or_else(|| format!("there is no scope {key}"))?;
    if !scopes::readable(directory, machine, &scope) {
        assets.gone(key, machine);
        return Err(format!("none of this machine's accounts reads the scope {key}"));
    }
    assets.holds(key, machine);
    Ok(scope)
}

/// The scope `key` and `stream` of it, through the read gate and the
/// stream's class.
fn held(assets: &Assets, directory: &Indexed<'_>, machine: &MachineName, key: &str, stream: &str) -> Result<Scope, String> {
    let scope = gated(assets, directory, machine, key)?;
    if !scopes::holds(directory, machine, &scope, stream) {
        return Err(format!("this machine does not hold {stream} of the scope {key}"));
    }
    Ok(scope)
}

/// An answer about the scope `key` made by the directory as it stood when
/// the request began, judged again by the directory as it stands now: one
/// the machine may no longer have is answered `Revoked` (design §6.3).
pub(crate) fn recheck(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &str,
    stream: Option<&str>,
    reply: Reply,
) -> Result<Reply, String> {
    if matches!(reply, Reply::Revoked { .. } | Reply::Sealed { .. }) {
        return Ok(reply);
    }
    if let Some(why) = assets.sealed() {
        return Ok(Reply::Sealed { why });
    }
    let Some(scope) = scopes::scope(assets, key)? else { return Ok(reply) };
    if !scopes::readable(directory, machine, &scope)
        || !scopes::sharers_read(directory, machine, &scope)
        || stream.is_some_and(|stream| !scopes::holds(directory, machine, &scope, stream))
    {
        assets.gone(key, machine);
        return Ok(Reply::Revoked { scope: key.to_string() });
    }
    Ok(reply)
}

/// The stream a request is of, if it is of one.
pub(crate) fn stream_of(request: &Request) -> Option<&str> {
    match request {
        Request::Frames { stream, .. } | Request::Succession { stream, .. } | Request::Epoch { stream, .. } => Some(stream),
        _ => None,
    }
}

/// The chain head `seen` carries, if the hub's word under its keyring
/// stands behind it.
fn heard_seen(assets: &Assets, seen: &wire::Seen) -> Option<(u64, [u8; 32])> {
    let head = seen.head.array()?;
    (assets.keys().heard(&seen.statement.0, &seen.word.0) == Some((seen.seq, head))).then_some((seen.seq, head))
}

/// What a reported receipt says the hub signed, the head it names, and the
/// hub's word on it.
fn receipt_head(receipt: &wire::Reported) -> Result<(Vec<u8>, Head, Vec<u8>), String> {
    let digest = array(&receipt.digest, "a receipt's digest")?;
    let frame = FrameKey { scope: &receipt.scope, stream: &receipt.stream, epoch: receipt.epoch, first: receipt.first, digest: &digest };
    Ok(match &receipt.said {
        wire::Said::Applied { change, prefix } => {
            let prefix = array(prefix, "a receipt's prefix")?;
            let head = Head { scope: receipt.scope.clone(), change: *change, prefix, parked: 0, accumulated: [0; 32] };
            (applied_statement(&frame, *change, &prefix), head, receipt.word.0.clone())
        }
        wire::Said::Parked { sequence, accumulated } => {
            let accumulated = array(accumulated, "a receipt's accumulator")?;
            let head = Head { scope: receipt.scope.clone(), change: 0, prefix: [0; 32], parked: *sequence, accumulated };
            (parked_statement(&frame, *sequence, &accumulated), head, receipt.word.0.clone())
        }
    })
}

/// The scope a request is of, if it is of one already described.
pub(crate) fn scope_of(request: &Request) -> Option<&str> {
    match request {
        Request::Open { .. } | Request::Describe { .. } | Request::Cleared { .. } => None,
        Request::Answer { scope, .. }
        | Request::Take { scope, .. }
        | Request::Cursors { scope, .. }
        | Request::Frames { scope, .. }
        | Request::Streams { scope }
        | Request::Epoch { scope, .. }
        | Request::Succession { scope, .. }
        | Request::Put { scope, .. }
        | Request::Get { scope, .. } => Some(scope),
    }
}

/// Answers `request` from `machine`, whose key is `key`, against the
/// directory `directory`. Call on a thread that may block.
pub(crate) fn ask(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &PublicKey,
    request: Request,
) -> Result<Reply, String> {
    if let Some(why) = assets.sealed() {
        // What a machine brings while sealed, as far as the hub's word stands
        // behind it, is kept for the recovery.
        if let Request::Open { seen, reported, .. } = &request {
            let seen = seen.as_ref().and_then(|seen| heard_seen(assets, seen));
            let receipts = reported.iter().filter_map(|receipt| receipt_head(receipt).ok()).filter(|(statement, _, word)| assets.keys().heard(statement, word).is_some()).map(|(_, head, _)| head).collect();
            assets.brought(machine, seen, receipts);
        }
        return Ok(Reply::Sealed { why });
    }
    // A scope this machine reads no more is answered so, whatever was asked
    // of it: its device stops using what it holds (§4.5, §6.3).
    if let Some(key) = scope_of(&request)
        && let Some(scope) = scopes::scope(assets, key)?
        && (!scopes::readable(directory, machine, &scope) || !scopes::sharers_read(directory, machine, &scope))
    {
        assets.gone(key, machine);
        return Ok(Reply::Revoked { scope: key.to_string() });
    }
    match request {
        Request::Open { version, seen, reported, pinned } => open(assets, directory, machine, key, version, seen, reported, pinned),
        Request::Describe { scope, readers, classes } => {
            Ok(Reply::Described { descriptor: scopes::describe(assets, directory, machine, &scope, readers, classes)? })
        }
        Request::Answer { scope, header, signature } => {
            let kept = gated(assets, directory, machine, &scope)?;
            let library = scopes::library(assets, &kept)?;
            let word = library
                .answer(&scope, &header.0, &signature.0, &DeviceKeys, assets.signer().as_ref())
                .map_err(|error| error.to_string())?;
            Ok(Reply::Word { received: word.map(received) })
        }
        Request::Take { scope, frame, proven } => {
            let kept = gated(assets, directory, machine, &scope)?;
            let library = scopes::library(assets, &kept)?;
            let period = library.holding(&scope).map_err(|error| error.to_string())?.ok_or("the scope is not held here")?;
            let proven: Vec<[u8; 32]> = proven.iter().map(|hash| array(hash, "a proven object's hash")).collect::<Result<_, _>>()?;
            // Only what this device sent every byte of lately, under its key
            // (§6.5).
            let proven = assets.objects.proven(key.as_bytes(), &proven);
            // An object the device reaches through a stream of the scope it
            // holds needs no proof; through the scope alone it does.
            let readable = |object: &ObjectRef| {
                library
                    .streams_reading(&scope, &object.hash)
                    .is_ok_and(|streams| streams.iter().any(|stream| scopes::holds(directory, machine, &kept, stream)))
            };
            let taking = Taking { period, readable: &readable, proven: &proven };
            let frame_header = frame.header.0.clone();
            let raw = RawFrame { header: frame.header.0, signature: frame.signature.0, body: frame.body.0, objects: Vec::new() };
            let word = library
                .take(&scope, raw, &DeviceKeys, &taking, Arc::new(Readers), assets.signer())
                .map_err(|error| error.to_string())?;
            if matches!(word, Received::Applied { .. } | Received::Parked { .. } | Received::Rejected { .. }) {
                assets.changed(kept.owner.clone(), &scope);
                assets.receipted(&scope, machine);
            }
            if matches!(word, Received::Applied { .. }) {
                let stream = Header::decode(&frame_header).map_err(|error| error.to_string())?.stream;
                let next = library
                    .streams(&scope)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|(name, _)| name == &stream)
                    .map_or(0, |(_, next)| next);
                assets.moved(&scope, &stream, next, machine);
            }
            Ok(Reply::Received { received: received(word) })
        }
        Request::Cleared { .. } => Err("a machine's word that it cleared is taken with the directory".into()),
        Request::Put { scope, object, prove, .. } => {
            let kept = gated(assets, directory, machine, &scope)?;
            super::objects::put(assets, machine, key, kept, &object, prove)
        }
        Request::Get { scope, object, from } => {
            let kept = gated(assets, directory, machine, &scope)?;
            super::objects::get(assets, directory, machine, key, kept, &object, from)
        }
        Request::Cursors { scope, holding, streams } => {
            let kept = gated(assets, directory, machine, &scope)?;
            let library = scopes::library(assets, &kept)?;
            let mut standing = Vec::with_capacity(streams.len());
            for cursor in streams {
                if !scopes::holds(directory, machine, &kept, &cursor.stream) {
                    return Err(format!("this machine does not hold {} of the scope {scope}", cursor.stream));
                }
                let head = array(&cursor.head, "a cursor's chain value")?;
                let said = library
                    .check_cursor(key.as_bytes(), &scope, &cursor.stream, (holding, cursor.sequence, head))
                    .map_err(|error| error.to_string())?;
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
        Request::Frames { scope, stream, from, limit } => {
            let kept = held(assets, directory, machine, &scope, &stream)?;
            let library = scopes::library(assets, &kept)?;
            let exported =
                library.export(&scope, &stream, from, limit.min(FRAMES_AT_ONCE) as usize).map_err(|error| error.to_string())?;
            let (mut frames, mut bytes) = (Vec::new(), 0usize);
            for frame in exported {
                bytes += frame.header.len() + frame.signature.len() + frame.body.len();
                if !frames.is_empty() && bytes > FRAME_BYTES_AT_ONCE {
                    break;
                }
                frames.push(wire::Frame { header: frame.header.into(), signature: frame.signature.into(), body: frame.body.into() });
            }
            Ok(Reply::Frames { frames })
        }
        Request::Streams { scope } => {
            let kept = gated(assets, directory, machine, &scope)?;
            let library = scopes::library(assets, &kept)?;
            let (held, withheld): (Vec<_>, Vec<_>) = library
                .streams(&scope)
                .map_err(|error| error.to_string())?
                .into_iter()
                .partition(|(stream, _)| scopes::holds(directory, machine, &kept, stream));
            let streams = held.into_iter().map(|(stream, next)| wire::StreamAt { stream, next }).collect();
            let withheld = withheld.into_iter().map(|(stream, _)| stream).collect();
            Ok(Reply::Streams { streams, withheld })
        }
        Request::Epoch { scope, stream, next, head } => {
            let kept = held(assets, directory, machine, &scope, &stream)?;
            let library = scopes::library(assets, &kept)?;
            // Only the device that orders the stream now opens its next epoch
            // itself; another takes it over by a handover the hub decides.
            let last = library.last_epoch(&scope, &stream).map_err(|error| error.to_string())?;
            if last.is_none_or(|(_, device)| &device != key.as_bytes()) {
                return Err("only the device that orders the stream opens its next epoch".into());
            }
            let successor = Successor { device: *key.as_bytes(), key: *key.as_bytes(), kind: EpochKind::Source };
            let cutoff = Cutoff { next, head: array(&head, "a chain value")? };
            let (signed, _) = library
                .handover(&scope, &stream, successor, cutoff, Arc::new(Readers), assets.signer())
                .map_err(|error| error.to_string())?;
            assets.changed(kept.owner.clone(), &scope);
            // In the hub's chain and outside its directory before the device
            // hears of it (design §6.1, §17).
            let epoch = fastctx_content::v2::EpochFact::decode(&signed.fact)?.epoch;
            assets.opened_epoch(&kept.owner, &scope, &stream, epoch, &signed.fact).map_err(|error| error.to_string())?;
            // The word given is bound to the head the epoch went into.
            let word = assets.signer().sign(&signed.fact);
            Ok(Reply::Epoch { descriptor: wire::Fact { fact: signed.fact.into(), word: word.into() } })
        }
        Request::Succession { scope, stream } => {
            let kept = held(assets, directory, machine, &scope, &stream)?;
            let library = scopes::library(assets, &kept)?;
            let (epochs, gaps) = library.succession(&scope, &stream).map_err(|error| error.to_string())?;
            // Each in the hub's chain and outside its directory before it is
            // handed out, and given with a word bound to that head.
            assets.witness_facts(&kept.owner, &scope, &stream, &epochs, &gaps).map_err(|error| error.to_string())?;
            let signer = assets.signer();
            let fact = |signed: fastctx_content::v2::Signed| {
                let word = signer.sign(&signed.fact);
                wire::Fact { fact: signed.fact.into(), word: word.into() }
            };
            Ok(Reply::Succession { epochs: epochs.into_iter().map(fact).collect(), gaps: gaps.into_iter().map(fact).collect() })
        }
    }
}

/// Opens a session (design §7.1, §17 item 4): the chain head the device
/// last saw, and every receipt it reports, checked against the hub's own
/// history. One the hub's word stands behind that the hub's history no
/// longer holds means the hub was put back: every asset is sealed from
/// then on. Receipts a checkpoint covers are answered with its word.
#[allow(clippy::too_many_arguments)]
fn open(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    key: &PublicKey,
    version: u32,
    seen: Option<wire::Seen>,
    reported: Vec<wire::Reported>,
    pinned: Option<Bytes>,
) -> Result<Reply, String> {
    if version != wire::VERSION {
        return Err(format!("asset sync {version} is not this hub's {}", wire::VERSION));
    }
    assets.speaks(key).map_err(|error| error.to_string())?;
    if let Some(pinned) = &pinned {
        let pinned = array(pinned, "a pinned key")?;
        // Only a key the hub's own word was once given under counts: a
        // machine's say-so alone is no evidence of a lost key.
        let shown = seen.as_ref().is_some_and(|seen| {
            fastctx_net::sign::heard_word(&PublicKey::from_bytes(pinned), &seen.statement.0, &seen.word.0).is_some()
        });
        if !assets.keys().holds(&pinned) && !shown {
            return Err("this machine pinned a service key this hub's word was never given under".into());
        }
        if !assets.keys().holds(&pinned) {
            let why = format!(
                "{machine} pinned a service key of the hub's ({}) its keyring lacks: the keyring was put back to before a rotation; \
                 bring back a keyring export that holds it",
                super::witness::hex(&pinned[..8])
            );
            assets.seal(&why);
            return Ok(Reply::Sealed { why });
        }
    }
    let public = assets.public();
    let (seq, head) = assets.witness.current();
    // What is of a history put aside when the hub recovered is handled:
    // the machine is told so, not taken for a new rollback.
    let mut closed = false;
    if let Some(seen) = seen {
        let seen_head = array(&seen.head, "a chain head")?;
        // Only a head the hub's own word carried counts, under any key of
        // its keyring.
        if assets.keys().heard(&seen.statement.0, &seen.word.0) == Some((seen.seq, seen_head)) {
            let kept = assets.control.read_now(move |c| chain::head_at(c, seen.seq)).map_err(|error| error.to_string())?;
            if kept != Some(seen_head) {
                if assets.closed_head(seen.seq, &seen_head).map_err(|error| error.to_string())? {
                    closed = true;
                } else {
                    let why = format!("{machine} saw the hub's history at {} that the hub does not hold", seen.seq);
                    assets.seal(&why);
                    return Ok(Reply::Sealed { why });
                }
            }
        }
    }
    let mut checkpointed = Vec::new();
    for receipt in reported {
        let Some(scope) = scopes::scope(assets, &receipt.scope)? else { continue };
        if !scopes::readable(directory, machine, &scope) {
            continue;
        }
        let (statement, wanted, word) = receipt_head(&receipt)?;
        if assets.keys().heard(&statement, &word).is_none() {
            continue;
        }
        let library = scopes::library(assets, &scope)?;
        if !library.holds(&wanted).map_err(|error| error.to_string())? {
            if assets.closed_receipt(&wanted).map_err(|error| error.to_string())? {
                closed = true;
                continue;
            }
            let why = format!("{machine} holds the hub's receipt of {} that the hub's history no longer holds", receipt.scope);
            assets.seal(&why);
            return Ok(Reply::Sealed { why });
        }
        if let Some(checkpoint) = covering(assets, &scope, &wanted)? {
            checkpointed.push(checkpoint);
        }
    }
    let rotations = assets
        .keys()
        .rotations()
        .iter()
        .map(|rotation| wire::Rotation { old: rotation.old.into(), new: rotation.new.into(), word: rotation.signature.clone().into() })
        .collect();
    Ok(Reply::Ready {
        key: public.as_bytes().to_vec().into(),
        clock: jiff::Timestamp::now().as_millisecond(),
        seq,
        head: head.into(),
        checkpointed,
        rotations,
        closed,
    })
}

/// The hub's word that a checkpoint covers `head` of `scope`, if one does:
/// only one the witness outside its directory holds already (design §17).
fn covering(assets: &Assets, scope: &Scope, head: &Head) -> Result<Option<wire::Checkpoint>, String> {
    let (owner, key, change, parked) = (scope.owner.key(), head.scope.clone(), head.change as i64, head.parked as i64);
    let witnessed = assets.witness.current().0 as i64;
    let found: Option<(i64, i64, i64)> = assets
        .control
        .read_now(move |c| {
            Ok(c.prepare_cached(
                "SELECT seq, change, parked FROM hub_checkpoints WHERE owner = ?1 AND scope = ?2 AND change >= ?3 AND parked >= ?4
                 AND seq <= ?5",
            )?
            .query_row(params![owner, key, change, parked, witnessed], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .optional()?)
        })
        .map_err(|error| error.to_string())?;
    let Some((seq, change, parked)) = found else { return Ok(None) };
    let at = seq as u64;
    let Some(chain_head) = assets.control.read_now(move |c| chain::head_at(c, at)).map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let statement = wire::checkpoint_statement(&head.scope, change as u64, parked as u64, at, &chain_head);
    Ok(Some(wire::Checkpoint {
        scope: head.scope.clone(),
        change: change as u64,
        parked: parked as u64,
        seq: at,
        head: chain_head.into(),
        word: assets.signer().sign(&statement).into(),
    }))
}
