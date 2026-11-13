//! What the hub answers a device's asset requests (design §6.4–§6.6, §7,
//! §8, §17). Every answer goes through the read gate first: the scope is
//! described, and one of the accounts behind the request reads it in the
//! directory as it stands; a stream is given only to a device its class
//! lets hold it. A hub that found it was put back answers nothing of
//! assets but why.

use super::scopes::{self, Scope};
use super::{Assets, chain};
use fastctx_content::v2::{
    Cutoff, EpochKind, FrameKey, Head, Header, RawFrame, Received, Standing, Successor, Taking,
    Verifier, applied_statement, parked_statement,
};
use fastctx_content::ObjectRef;
use fastctx_net::authorization::Lookup as _;
use fastctx_net::directory_index::Indexed;
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::MachineName;
use fastctx_types::assets::{self as wire, Bytes, Reply, Request};
use std::sync::Arc;

/// Frames given in one answer at most.
const FRAMES_AT_ONCE: u32 = 64;
/// Streams one page of what changed in an owner library looks at.
const CHANGES_AT_ONCE: usize = 512;
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

/// Who may open the first epoch of `stream` of a conversation's scope, the
/// hub having none of it yet (AQ07): the device that orders the
/// conversation's timeline — where it lives now — or, before anything of it
/// is ordered, the machine it was born on. Another reader of the scope
/// cannot take a new stream first. Other scopes are not gated here.
fn first_epoch_gate(assets: &Assets, directory: &Indexed<'_>, kept: &Scope, scope: &str, stream: &str, device: &[u8; 32]) -> Result<(), String> {
    // A sentinel's wakes, begun by the machine its stream names — which the
    // kind's own rule holds every frame to — where one of the conversation's
    // sentinels lives on it as the hub keeps them (§29, 2.7).
    if stream.starts_with("sentinel-runs/") {
        return sentinel_runs_gate(assets, directory, kept, scope, stream, device);
    }
    // A conversation's scope, or a body's — a conversation opened on a
    // machine shared whole, or that machine's log — begun on its machine;
    // what an account does on a machine, on that machine; what a machine
    // did and spent for an account, by the machine its stream names (AS14).
    // A body's machine is the member whose body it is now: its key, its
    // owner and when that owner was opened make the body's name, so neither
    // one gone from the network nor another that took its name begins a
    // body's stream (AT06).
    let mut of_body = None;
    let born_on = match &kept.descriptor.readers {
        wire::Readers::Conversation { born_on, .. } => born_on.clone(),
        wire::Readers::Body { body, .. } => {
            of_body = Some(body.clone());
            match &kept.bodies {
                Some(bodies) => bodies.machine(body).map_err(|error| error.to_string())?,
                None => None,
            }
        }
        wire::Readers::Working { machine, .. } => Some(machine.clone()),
        wire::Readers::Account { .. } => {
            let Some((_, named)) = stream.split_once('/') else { return Ok(()) };
            let library = scopes::library(assets, kept)?;
            if library.last_epoch(scope, stream).map_err(|error| error.to_string())?.is_some() {
                return Ok(());
            }
            let hex: String = device[..8].iter().map(|byte| format!("{byte:02x}")).collect();
            return if named == hex { Ok(()) } else { Err(format!("{stream} of {scope} is begun by the machine it names")) };
        }
        _ => return Ok(()),
    };
    let library = scopes::library(assets, kept)?;
    if library.last_epoch(scope, stream).map_err(|error| error.to_string())?.is_some() {
        return Ok(());
    }
    // A body's machine is the one it was kept as, though its owner changed
    // since (§29, 2.1).
    let of_its_body = |member: &fastctx_net::directory::Member| match (&of_body, &kept.bodies) {
        (None, _) => true,
        (Some(body), Some(bodies)) => bodies.is_of(directory.all, body, &member.name).unwrap_or(false),
        (Some(body), None) => directory.all.body_of(&member.name).as_ref() == Some(body),
    };
    // The machine it was born on by any name it had; one gone from the
    // network begins nothing, and another reader does not begin it in its
    // stead: the account that opened it adopts it (§29, 2.3; codex-5c30d5e9
    // B07). A conversation born assisting an account names no machine, and
    // whichever of its readers orders it first begins it.
    let source = match library.last_epoch(scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM).map_err(|error| error.to_string())? {
        Some((_, orderer)) => orderer,
        None => match born_on.as_ref().map(|machine| directory.all.find_member(machine)) {
            Some(Some(member)) if of_its_body(member) => *member.key.as_bytes(),
            _ if of_body.is_some() => return Err(format!("{stream} of {scope} is begun by its body's machine, which is not here")),
            Some(_) => {
                return Err(format!(
                    "{stream} of {scope} is begun by the machine it was born on, which is not here: the account that opened it adopts it"
                ));
            }
            None => return Ok(()),
        },
    };
    // The device the frame says made it, whose signature `take` checks:
    // one handing on its predecessor's first frame hands on the
    // predecessor's (AR06).
    if source != *device {
        return Err(format!("{stream} of {scope} is begun by the machine the conversation lives on"));
    }
    Ok(())
}

/// Whether `device` may begin `stream`, a sentinel's wakes, of the
/// conversation's scope `scope`: one of the conversation's sentinels as the
/// hub keeps them (the `sentinels` stream it orders into the scope) lives
/// on it, or lived on it before it moved on (`sentinel_homes`).
fn sentinel_runs_gate(assets: &Assets, directory: &Indexed<'_>, kept: &Scope, scope: &str, stream: &str, device: &[u8; 32]) -> Result<(), String> {
    let hex: String = device[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    if stream.strip_prefix("sentinel-runs/") != Some(hex.as_str()) {
        return Err(format!("{stream} of {scope} is written by the machine it names"));
    }
    let library = scopes::library(assets, kept)?;
    if library.last_epoch(scope, stream).map_err(|error| error.to_string())?.is_some() {
        return Ok(());
    }
    let sentinels = <fastctx_content::v2::ConversationSentinel as fastctx_content::v2::Kind>::STREAM;
    let mut after = None;
    loop {
        let page = library.page(scope, sentinels, after.take(), 256).map_err(|error| error.to_string())?;
        let full = page.len() == 256;
        for entry in &page {
            let home = sentinel_record(&library, entry)
                .and_then(|record| record["body"]["definition"]["home"].as_str().map(str::to_string))
                .and_then(|home| fastctx_types::MachineName::try_from(home).ok());
            if home.and_then(|home| directory.all.find_member(&home)).is_some_and(|member| member.key.as_bytes() == device) {
                return Ok(());
            }
        }
        match page.last() {
            Some(last) if full => after = Some((last.order.clone(), last.item.clone())),
            _ => break,
        }
    }
    // Or lived on: its home before it moved on (freeze spec §六, now or ever).
    for home in super::sentinel_homes::ever(assets, scope)? {
        let found = fastctx_types::MachineName::try_from(home).ok().and_then(|home| directory.all.find_member(&home).map(|member| *member.key.as_bytes()));
        if found.is_some_and(|key| key == *device) {
            return Ok(());
        }
    }
    Err(format!("{stream} of {scope} is begun by a machine one of the conversation's sentinels lives or lived on"))
}

/// A sentinel's account record as the scope holds it: its body, or the one
/// object it is in (`mirror::fitted`).
fn sentinel_record(library: &fastctx_content::v2::Library, entry: &fastctx_content::v2::Entry) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(&entry.body).ok()?;
    if value.get("in_object").and_then(serde_json::Value::as_bool) != Some(true) {
        return Some(value);
    }
    let [object] = entry.objects.as_slice() else { return None };
    let (mut bytes, mut at) = (Vec::new(), 0u64);
    while at < object.size {
        let piece = library.read(&object.hash, at, 1 << 20).ok()?;
        if piece.is_empty() {
            return None;
        }
        at += piece.len() as u64;
        bytes.extend_from_slice(&piece);
    }
    serde_json::from_slice(&bytes).ok()
}

/// The kinds the hub takes records of, each judged by its own rule (design
/// §11); a record of a kind not registered is refused.
pub(crate) fn kinds() -> Arc<fastctx_content::v2::Kinds> {
    let kinds = fastctx_content::v2::Kinds::new()
        .register::<fastctx_content::v2::Timeline>()
        .register::<fastctx_content::v2::ConversationMeta>()
        .register::<fastctx_content::v2::ConversationWords>()
        .register::<fastctx_content::v2::ConversationAttachment>()
        .register::<fastctx_content::v2::JobFacts>()
        .register::<fastctx_content::v2::JobOutput>()
        .register::<fastctx_content::v2::ConversationPlan>()
        .register::<fastctx_content::v2::ConversationHistory>()
        .register::<fastctx_content::v2::ConversationGoal>()
        .register::<fastctx_content::v2::ConversationSentinel>()
        .register::<fastctx_content::v2::AccountSettings>()
        .register::<fastctx_content::v2::AccountSkill>()
        .register::<fastctx_content::v2::ProjectLedger>()
        .register::<fastctx_content::v2::MachineAudit>()
        .register::<fastctx_content::v2::AccountNotice>()
        .register::<fastctx_content::v2::MachineLog>()
        .register::<fastctx_content::v2::McpLog>()
        .register::<fastctx_content::v2::HubLog>()
        .register::<fastctx_content::v2::ConversationTimer>()
        .register::<fastctx_content::v2::ConversationSentinelRun>()
        .register::<fastctx_content::v2::ConversationArtifact>()
        .register::<fastctx_content::v2::AccountProfile>()
        .register::<fastctx_content::v2::AccountCollaboration>()
        .register::<fastctx_content::v2::AccountActivity>()
        .register::<fastctx_content::v2::AccountUsage>()
        .register::<fastctx_content::v2::ConversationDraft>();
    #[cfg(test)]
    let kinds = kinds.register::<super::drill_kinds::Plain>();
    Arc::new(kinds)
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

/// Whether an account that opened `scope` is logged in on `machine`: such
/// an account may give what it opened to a machine of its own, the device it
/// lived on lost (design §28.10). The account as it was opened, the
/// descriptor's `born` says: one made again under its name since, though a
/// member of the body now, did not open it.
fn opened_here(directory: &Indexed<'_>, machine: &MachineName, scope: &Scope) -> bool {
    use fastctx_types::assets::Readers;
    let openers: Vec<&fastctx_types::AccountName> = match &scope.descriptor.readers {
        Readers::Conversation { openers, .. } => openers.iter().collect(),
        Readers::Account { account } => vec![account],
        Readers::Body { opener, .. } => vec![opener],
        Readers::Working { account, .. } => vec![account],
        Readers::Project { .. } | Readers::Hub => Vec::new(),
    };
    let as_born = |account: &fastctx_types::AccountName| {
        let opened = directory.account(account).map(|found| found.opened.as_millisecond().to_string());
        opened.is_some_and(|opened| scope.descriptor.born.get(&format!("account:{account}")) == Some(&opened))
    };
    openers.into_iter().any(|account| directory.logged_in(machine, account) && as_born(account))
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

/// An answer of what `machine` may hold, judged again by the directory as
/// it stands now (`holdings::recheck`).
pub(crate) fn recheck_holdings(assets: &Assets, directory: &Indexed<'_>, now: &str, machine: &MachineName, reply: Reply) -> Result<Reply, String> {
    super::holdings::recheck(assets, directory, now, machine, reply)
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
    // An answer about several streams, each judged again.
    Ok(match reply {
        Reply::Streams { streams, mut withheld } => {
            let (held, gone): (Vec<_>, Vec<_>) = streams.into_iter().partition(|at| scopes::holds(directory, machine, &scope, &at.stream));
            withheld.extend(gone.into_iter().map(|at| at.stream));
            Reply::Streams { streams: held, withheld }
        }
        Reply::Standing { streams } => {
            if let Some(gone) = streams.iter().find(|standing| !scopes::holds(directory, machine, &scope, &standing.stream)) {
                return Err(format!("this machine does not hold {} of the scope {key}", gone.stream));
            }
            Reply::Standing { streams }
        }
        reply => reply,
    })
}

/// The stream a request is of, if it is of one.
pub(crate) fn stream_of(request: &Request) -> Option<&str> {
    // Every request named: one added later is judged here before it is
    // answered, not passed by (AL03).
    match request {
        Request::Frames { stream, .. }
        | Request::Succession { stream, .. }
        | Request::Epoch { stream, .. }
        | Request::Takeover { stream, .. }
        | Request::Window { stream, .. }
        | Request::Older { stream, .. } => Some(stream),
        Request::Open { .. }
        | Request::Describe { .. }
        | Request::Cleared { .. }
        | Request::Holdings { .. }
        | Request::Changes { .. }
        | Request::Answer { .. }
        | Request::Take { .. }
        | Request::Stage { .. }
        | Request::Cursors { .. }
        | Request::Streams { .. }
        | Request::Put { .. }
        | Request::Get { .. }
        | Request::Report { .. }
        | Request::SyncStatus { .. } => None,
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

/// What `machine` brings that shows the hub was put back, as an ordinary
/// `open` would find it: a chain head the hub's word carried that its chain
/// does not hold, or the hub's receipt of what its libraries no longer
/// hold, neither of a history the hub set aside as it recovered. Why, if so.
fn put_back(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    seen: Option<&wire::Seen>,
    reported: &[wire::Reported],
    pinned: Option<&Bytes>,
    lineage: &[wire::Rotation],
) -> Result<Option<String>, String> {
    // A pinned key that shows the keyring was put back counts as an
    // ordinary open finds it (AJ02); one nothing of the keyring leads to
    // shows nothing here.
    if let Ok(Some(why)) = keyring_behind(assets, machine, seen, pinned, lineage) {
        return Ok(Some(why));
    }
    if let Some((seq, head)) = seen.and_then(|seen| heard_seen(assets, seen)) {
        let kept = assets.control.read_now(move |c| chain::head_at(c, seq)).map_err(|error| error.to_string())?;
        if kept != Some(head) && !assets.closed_head(seq, &head).map_err(|error| error.to_string())? {
            return Ok(Some(format!("{machine} saw the hub's history at {seq} that the hub does not hold")));
        }
    }
    for receipt in reported {
        let Some(scope) = scopes::scope(assets, &receipt.scope)? else { continue };
        if !scopes::readable(directory, machine, &scope) {
            continue;
        }
        let Ok((statement, wanted, word)) = receipt_head(receipt) else { continue };
        if assets.keys().heard(&statement, &word).is_none() {
            continue;
        }
        let library = scopes::library(assets, &scope)?;
        if !library.holds(&wanted).map_err(|error| error.to_string())?
            && !assets.closed_receipt(&wanted).map_err(|error| error.to_string())?
        {
            return Ok(Some(format!("{machine} holds the hub's receipt of {} that the hub's history no longer holds", receipt.scope)));
        }
    }
    Ok(None)
}

/// What the key `machine` pinned for the hub shows of the hub's keyring
/// (design §17): a key the keyring's own signed rotations lead to that it
/// lacks, or its next key the hub was heard speaking under, means the
/// keyring was put back to before a rotation. Why, if so. A pinned key no
/// rotation of the keyring leads to is refused.
fn keyring_behind(
    assets: &Assets,
    machine: &MachineName,
    seen: Option<&wire::Seen>,
    pinned: Option<&Bytes>,
    lineage: &[wire::Rotation],
) -> Result<Option<String>, String> {
    let Some(pinned) = pinned else { return Ok(None) };
    let pinned = array(pinned, "a pinned key")?;
    let keys = assets.keys();
    if !keys.holds(&pinned) {
        // Only a key the keyring's own signed rotations lead to counts: a
        // key a machine made, signing for itself, shows nothing.
        let lineage: Vec<_> = lineage.iter().map(|rotation| (rotation.old.0.clone(), rotation.new.0.clone(), rotation.word.0.clone())).collect();
        if !keys.descends(&pinned, &lineage) {
            return Err("this machine pinned a service key no rotation of this hub's keyring leads to".into());
        }
        return Ok(Some(format!(
            "{machine} pinned a service key of the hub's ({}) its keyring lacks: the keyring was put back to before a rotation; \
             bring back a keyring export that holds it",
            super::witness::hex(&pinned[..8])
        )));
    }
    if keys.is_next(&pinned) {
        // Only a machine that heard the hub speak under it shows it was put
        // in use: the next key's public half was given out with its
        // rotation before.
        let spoken = seen.is_some_and(|seen| {
            fastctx_net::sign::heard_word(&PublicKey::from_bytes(pinned), &seen.statement.0, &seen.word.0).is_some()
        });
        if !spoken {
            return Err("this machine pinned the keyring's next service key, which the hub never spoke under".into());
        }
        return Ok(Some(format!(
            "{machine} pinned the keyring's next service key ({}): the rotation was put in use after this keyring was exported; \
             put it in use with `fastctx hub rotate-key`, then reopen",
            super::witness::hex(&pinned[..8])
        )));
    }
    Ok(None)
}

/// The scope a request is of, if it is of one already described.
pub(crate) fn scope_of(request: &Request) -> Option<&str> {
    match request {
        Request::Open { .. }
        | Request::Describe { .. }
        | Request::Cleared { .. }
        | Request::Holdings { .. }
        | Request::Changes { .. }
        | Request::Report { .. }
        | Request::SyncStatus { .. } => None,
        Request::Answer { scope, .. }
        | Request::Take { scope, .. }
        | Request::Stage { scope, .. }
        | Request::Cursors { scope, .. }
        | Request::Frames { scope, .. }
        | Request::Streams { scope }
        | Request::Epoch { scope, .. }
        | Request::Takeover { scope, .. }
        | Request::Succession { scope, .. }
        | Request::Window { scope, .. }
        | Request::Older { scope, .. }
        | Request::Put { scope, .. }
        | Request::Get { scope, .. } => Some(scope),
    }
}

/// Answers `request` from `machine`, whose key is `key`, against the
/// directory `directory`. Call on a thread that may block.
pub(crate) fn ask(
    assets: &Assets,
    directory: &Indexed<'_>,
    basis: &dyn Fn() -> String,
    machine: &MachineName,
    key: &PublicKey,
    request: Request,
) -> Result<Reply, String> {
    if let Some(why) = assets.sealed() {
        // What a machine brings while sealed, as far as the hub's word stands
        // behind it, is kept for the recovery.
        if let Request::Open { seen, reported, pinned, lineage, .. } = &request {
            let heard = seen.as_ref().and_then(|seen| heard_seen(assets, seen));
            let receipts: Vec<_> = reported.iter().filter_map(|receipt| receipt_head(receipt).ok()).filter(|(statement, _, word)| assets.keys().heard(statement, word).is_some()).map(|(_, head, _)| head).collect();
            let _kept_at = assets.brought(machine, heard, receipts.clone());
            // Sealed only as it started when the machine came, the hub that
            // is shown it was put back seals for its recovery instead: the
            // super administrator can no longer open it as an ordinary
            // start. Opened meanwhile, it is sealed all the same: what the
            // machine brought stands either way (AJ01).
            if Assets::is_start_seal(&why) {
                match put_back(assets, directory, machine, seen.as_ref(), reported, pinned.as_ref(), lineage)? {
                    Some(found) => {
                        assets.seal(&found);
                        return Ok(Reply::Sealed { why: found });
                    }
                    // Judged to show nothing: only what is judged so goes
                    // as the seal is opened (AO01).
                    None => super::brought::judged(assets, machine, heard, &receipts),
                }
            }
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
        Request::Open { version, seen, reported, pinned, lineage } => open(assets, directory, machine, key, version, seen, reported, pinned, lineage),
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
        Request::Holdings { after } => super::holdings::list(assets, directory, machine, after, basis()),
        Request::Stage { scope, size } => {
            let kept = gated(assets, directory, machine, &scope)?;
            super::objects::stage(assets, machine, key, kept, size)
        }
        Request::Take { scope, frame, proven: _, staged } => {
            let kept = gated(assets, directory, machine, &scope)?;
            // The streams the hub orders itself take no frame a device
            // brings, its own given back among them: after a recovery the hub
            // writes them again from its records, and a replica follows it
            // (design §28.17; AS02, AS07).
            if Header::decode(&frame.header.0).is_ok_and(|header| fastctx_content::v2::hub_orders(&header.stream)) {
                return Err("the hub orders that stream itself".into());
            }
            // A stream's first epoch is opened by its source alone (AQ07),
            // signed by the very device it names (AS01).
            if let Ok(header) = Header::decode(&frame.header.0) {
                if header.key != header.device {
                    return Err("a frame is signed by the device it names".into());
                }
                first_epoch_gate(assets, directory, &kept, &scope, &header.stream, &header.device)?;
            }
            // A body too large for the control connection was staged apart
            // under its ticket (§7.1), and stays there while the frame
            // waits for objects.
            let body = if staged != 0 {
                if !frame.body.0.is_empty() {
                    return Err("a frame staged apart comes without its body".into());
                }
                let body = match assets.objects.apart.body(staged, key.as_bytes()) {
                    Ok(taken) => taken,
                    // Settled already, its body let go: the same frame asked
                    // again hears what the hub said of it.
                    Err(why) => {
                        let library = scopes::library(assets, &kept)?;
                        let word = library
                            .answer(&scope, &frame.header.0, &frame.signature.0, &DeviceKeys, assets.signer().as_ref())
                            .map_err(|error| error.to_string())?;
                        return match word {
                            Some(word) => Ok(Reply::Received { received: received(word) }),
                            None => Err(why),
                        };
                    }
                };
                if body.0.len() as u64 != frame.apart {
                    assets.objects.apart.untake(staged, key.as_bytes(), body.1);
                    return Err("the body staged under that ticket is not the frame's".into());
                }
                ((*body.0).clone(), body.1)
            } else if frame.too_large() || frame.apart != 0 {
                return Err(format!("a frame body over {} bytes travels apart", wire::INLINE_FRAME_BYTES));
            } else {
                (frame.body.0, 0)
            };
            let (body, take) = body;
            // However this take ends without a word on the frame, its staged
            // body waits for the frame asked again (`touch` and `release`
            // settle it otherwise first).
            struct Untake<'a>(&'a super::apart::Apart, u64, [u8; 32], u64);
            impl Drop for Untake<'_> {
                fn drop(&mut self) {
                    if self.1 != 0 {
                        self.0.untake(self.1, &self.2, self.3);
                    }
                }
            }
            let _untake = Untake(&assets.objects.apart, staged, *key.as_bytes(), take);
            let library = scopes::library(assets, &kept)?;
            let period = library.holding(&scope).map_err(|error| error.to_string())?.ok_or("the scope is not held here")?;
            // Only what this device sent every byte of lately, under its key
            // (§6.5), whether the request names it again or not: looked up
            // for each object the frame refers to (AC06).
            let device_key = *key.as_bytes();
            let proven = |hash: &[u8; 32]| assets.objects.proven(&device_key, hash);
            // An object the device reaches through a stream of the scope it
            // holds needs no proof; through the scope alone it does.
            let readable = |object: &ObjectRef| {
                library
                    .streams_reading(&scope, &object.hash)
                    .is_ok_and(|streams| streams.iter().any(|stream| scopes::holds(directory, machine, &kept, stream)))
            };
            let taking = Taking { period, readable: &readable, proven: &proven };
            let frame_header = frame.header.0.clone();
            // What it refers to: once applied, read through it, no longer
            // proved (AF04).
            let referred: Vec<[u8; 32]> = Header::decode(&frame_header)
                .ok()
                .and_then(|header| fastctx_content::v2::open(&header, &body).ok())
                .map(|records| {
                    records
                        .iter()
                        .flat_map(|record| match &record.record.state {
                            fastctx_content::State::Is { objects, .. } => objects.iter().map(|object| object.hash).collect(),
                            fastctx_content::State::Gone => Vec::new(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let raw = RawFrame { header: frame.header.0, signature: frame.signature.0, body, objects: Vec::new() };
            let word = library
                .take(&scope, raw, &DeviceKeys, &taking, kinds(), assets.signer())
                .map_err(|error| error.to_string())?;
            fastctx_platform::lab::die_at("take", &scope);
            if staged != 0 {
                if matches!(word, Received::Missing(_) | Received::Prove(_)) {
                    assets.objects.apart.touch(staged, key.as_bytes());
                } else {
                    assets.objects.apart.release(staged, key.as_bytes());
                }
            }
            if matches!(word, Received::Applied { .. } | Received::Parked { .. } | Received::Rejected { .. }) {
                assets.changed(kept.owner.clone(), &scope);
                assets.receipted(&scope, machine);
            }
            // Kept by the hub, applied or parked, a frame needs its proofs no
            // more: asked again, it is answered as it was (AF04, AG02).
            if matches!(word, Received::Applied { .. } | Received::Parked { .. }) {
                assets.objects.forget_proofs(&device_key, &referred);
            }
            if matches!(word, Received::Applied { .. }) {
                let stream = Header::decode(&frame_header).map_err(|error| error.to_string())?.stream;
                let next = library
                    .streams(&scope)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|(name, _)| name == &stream)
                    .map_or(0, |(_, next)| next);
                assets.moved(&kept.owner, &scope, &stream, next, machine);
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
            let (mut frames, mut bytes, mut apart) = (Vec::new(), 0usize, Vec::new());
            for frame in exported {
                bytes += frame.header.len() + frame.signature.len() + frame.body.len();
                if !frames.is_empty() && bytes > FRAME_BYTES_AT_ONCE {
                    break;
                }
                let mut sent = wire::Frame::whole(frame.header, frame.signature, frame.body);
                // A body too large for the control connection follows on a
                // data connection (§7.1).
                if sent.too_large() {
                    sent.apart = sent.body.0.len() as u64;
                    apart.push(std::mem::take(&mut sent.body.0));
                }
                frames.push(sent);
            }
            let ticket = if apart.is_empty() {
                0
            } else {
                match super::objects::send_apart(assets, machine, key, kept, &stream, apart) {
                    Ok(ticket) => ticket,
                    Err(why) => {
                        // No room for the bodies now: the frames before the
                        // first of them go alone.
                        let first = frames.iter().position(|frame| frame.apart > 0).unwrap_or(0);
                        if first == 0 {
                            return Err(why);
                        }
                        frames.truncate(first);
                        0
                    }
                }
            };
            Ok(Reply::Frames { frames, ticket })
        }
        Request::Changes { owner, after, before, class, limit } => {
            // Only a library that is there is opened: asking never makes one.
            let Some(owned) = super::owners::Owner::from_key(&owner) else { return Err(format!("{owner} is not an owner's library")) };
            if !assets.has_library(&owned) {
                return Ok(Reply::Changes { generation: String::new(), streams: Vec::new(), top: 0, top_at: 0, through: after, more: false });
            }
            let library = assets.library(&owned).map_err(|error| error.to_string())?;
            let page = library.changes(class, after, before, (limit as usize).clamp(1, CHANGES_AT_ONCE)).map_err(|error| error.to_string())?;
            assets.follows(&owned, machine);
            // Each row through the read gate: one this machine may not hold
            // is stepped past, the page's place going on all the same.
            let mut streams = Vec::new();
            for row in page.rows {
                let Some(scope) = scopes::scope(assets, &row.scope)? else { continue };
                if !scopes::readable(directory, machine, &scope) || !scopes::holds(directory, machine, &scope, &row.stream) {
                    continue;
                }
                streams.push(wire::ChangedStream {
                    scope: row.scope,
                    stream: row.stream,
                    next: row.next,
                    head: row.head.into(),
                    bytes: row.bytes,
                    remade: row.remade,
                    at: row.at,
                    epoch: row.epoch,
                });
            }
            Ok(Reply::Changes { generation: page.generation, streams, top: page.top, top_at: page.top_at, through: page.through, more: page.more })
        }
        Request::Report { report } => super::standing::keep(assets, directory, machine, key.as_bytes(), report),
        // Answered beside the hub's sessions, where who is online is known.
        Request::SyncStatus { .. } => Err("a sync status is asked of the hub itself".into()),
        Request::Streams { scope } => {
            let kept = gated(assets, directory, machine, &scope)?;
            let library = scopes::library(assets, &kept)?;
            let (held, withheld): (Vec<_>, Vec<_>) = library
                .streams(&scope)
                .map_err(|error| error.to_string())?
                .into_iter()
                .partition(|(stream, _)| scopes::holds(directory, machine, &kept, stream));
            let mut streams = Vec::with_capacity(held.len());
            let remade = library.remade_of(&scope).map_err(|error| error.to_string())?;
            for (stream, next) in held {
                let epoch = library.last_epoch(&scope, &stream).map_err(|error| error.to_string())?.map_or(0, |(epoch, _)| epoch);
                let remade = remade.get(&stream).copied().unwrap_or(0);
                streams.push(wire::StreamAt { stream, next, epoch, remade });
            }
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
            // From the epoch it was judged the orderer of, none later (AG05).
            let seen = last.map(|(epoch, _)| epoch);
            let (signed, _) = library
                .handover_from(&scope, &stream, successor, cutoff, seen, kinds(), assets.signer())
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
        Request::Takeover { scope, stream, release, adopt } => {
            // A stream the hub orders is handed to no device (AR05, L17).
            if fastctx_content::v2::hub_orders(&stream) {
                return Err("the hub orders that stream itself".into());
            }
            let kept = held(assets, directory, machine, &scope, &stream)?;
            let library = scopes::library(assets, &kept)?;
            let device = *key.as_bytes();
            let signer = assets.signer();
            let word_of = |signed: &fastctx_content::v2::Signed| wire::Fact { fact: signed.fact.clone().into(), word: signer.sign(&signed.fact).into() };
            // Ordering it already: the epoch it orders in is the answer.
            let last = library.last_epoch(&scope, &stream).map_err(|error| error.to_string())?;
            if last.is_some_and(|(_, orderer)| orderer == device) {
                let (epochs, _) = library.succession(&scope, &stream).map_err(|error| error.to_string())?;
                let current = epochs.into_iter().last().ok_or("the stream's epochs are not here")?;
                assets.witness_facts(&kept.owner, &scope, &stream, std::slice::from_ref(&current), &[]).map_err(|error| error.to_string())?;
                return Ok(Reply::Epoch { descriptor: word_of(&current) });
            }
            // Only the device that orders it lets it go, to the machine its
            // release names; or the account that opened the scope gives it
            // to a machine of its own, logged in here (design §28.10).
            // Holding the stream is not enough.
            let me = directory.canonical(machine).to_string();
            let released = match (release, adopt) {
                (Some(given), _) => Some((
                    fastctx_content::v2::Release {
                        hub: assets.id.clone(),
                        scope: scope.clone(),
                        stream: stream.clone(),
                        epoch: given.epoch,
                        to: given.to.clone(),
                        to_key: array(&given.to_key, "a device key")?,
                        next: given.next,
                        head: array(&given.head, "a chain value")?,
                    },
                    given.signature.0,
                )),
                (None, true) => {
                    if !opened_here(directory, machine, &kept) {
                        return Err(format!("{scope} was opened by no account logged in on {me}: it is not this machine's to adopt"));
                    }
                    // Nothing of it ordered anywhere yet: the account that
                    // opened it begins it on its own machine (§29, 2.3, NN14).
                    if last.is_none() {
                        let successor = Successor { device, key: device, kind: EpochKind::Source };
                        let signed = library.open_epoch(&scope, &stream, successor, assets.signer()).map_err(|error| error.to_string())?;
                        assets.changed(kept.owner.clone(), &scope);
                        let epoch = fastctx_content::v2::EpochFact::decode(&signed.fact)?.epoch;
                        assets.opened_epoch(&kept.owner, &scope, &stream, epoch, &signed.fact).map_err(|error| error.to_string())?;
                        return Ok(Reply::Epoch { descriptor: word_of(&signed) });
                    }
                    None
                }
                (None, false) => {
                    return Err(format!("{stream} of {scope} is ordered by another device: taking it over needs that device's release, or the account that opened it adopting it here"));
                }
            };
            let verify = |key: &[u8; 32], statement: &[u8], signature: &[u8]| {
                let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
                fastctx_net::sign::PublicKey::from_bytes(*key)
                    .verify(fastctx_net::sign::Domain::AssetRelease, statement, &fastctx_net::sign::Signature::from_bytes(signature))
                    .is_ok()
            };
            let authority = match &released {
                Some((release, signature)) => fastctx_content::v2::Authority::Release { release, signature, verify: &verify },
                None => fastctx_content::v2::Authority::Adopt { from: last.map_or(0, |(epoch, _)| epoch) },
            };
            let successor = Successor { device, key: device, kind: EpochKind::Source };
            match library.move_stream(&scope, &stream, &me, successor, authority, kinds(), assets.signer()).map_err(|error| error.to_string())? {
                fastctx_content::v2::Moved::Handed { descriptor, .. } => {
                    assets.changed(kept.owner.clone(), &scope);
                    let epoch = fastctx_content::v2::EpochFact::decode(&descriptor.fact)?.epoch;
                    assets.opened_epoch(&kept.owner, &scope, &stream, epoch, &descriptor.fact).map_err(|error| error.to_string())?;
                    Ok(Reply::Epoch { descriptor: word_of(&descriptor) })
                }
                fastctx_content::v2::Moved::Superseded { epochs } => {
                    // Outside the hub's directory before the device hears of
                    // them, as every other answer's (AP05).
                    assets.witness_facts(&kept.owner, &scope, &stream, &epochs, &[]).map_err(|error| error.to_string())?;
                    Ok(Reply::HandedOn { epochs: epochs.iter().map(word_of).collect() })
                }
            }
        }
        Request::Window { scope, stream } => {
            let kept = held(assets, directory, machine, &scope, &stream)?;
            let library = scopes::library(assets, &kept)?;
            let window = library
                .window(&scope, &stream, fastctx_content::v2::WINDOW_ITEMS, fastctx_content::v2::WINDOW_BYTES)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{stream} of {scope} is not on the hub"))?;
            // In the hub's history outside its directory before it is given:
            // a library put back to before it is found out (II08).
            assets.checkpoint_now(&kept.owner, &scope).map_err(|error| error.to_string())?;
            // The hub's word on the whole of it, as of its head.
            let word = assets.signer().sign(&window.statement());
            Ok(Reply::Window {
                generation: window.generation,
                next: window.next,
                head: window.head.to_vec().into(),
                bytes: window.bytes,
                remade: window.remade,
                low: window.low.map(|(place, item)| (place.into(), item.into())),
                entries: window.entries.into_iter().map(wire_entry).collect(),
                word: word.into(),
            })
        }
        Request::Older { scope, stream, before, limit } => {
            let kept = held(assets, directory, machine, &scope, &stream)?;
            let library = scopes::library(assets, &kept)?;
            let before = before.map(|(place, item)| (place.0, item.0));
            let older = library
                .older(&scope, &stream, before, limit as usize)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{stream} of {scope} is not on the hub"))?;
            Ok(Reply::Older {
                through: older.through,
                entries: older.entries.into_iter().map(wire_entry).collect(),
                more: older.more,
                remade: older.remade,
            })
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

/// An item of a hub's index as it goes on the wire.
fn wire_entry(entry: fastctx_content::v2::Entry) -> wire::Entry {
    wire::Entry {
        item: entry.item.into(),
        order: entry.order.into(),
        revision: entry.revision,
        kind: entry.kind,
        schema: entry.schema,
        body: entry.body.into(),
        objects: entry.objects.into_iter().map(|object| (object.hash.to_vec().into(), object.size)).collect(),
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
    lineage: Vec<wire::Rotation>,
) -> Result<Reply, String> {
    if version != wire::VERSION {
        return Err(format!("asset sync {version} is not this hub's {}", wire::VERSION));
    }
    assets.speaks(key).map_err(|error| error.to_string())?;
    if let Some(why) = keyring_behind(assets, machine, seen.as_ref(), pinned.as_ref(), &lineage)? {
        assets.seal(&why);
        return Ok(Reply::Sealed { why });
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

#[cfg(test)]
mod tests {
    use fastctx_content::v2::{
        ConversationAttachment, ConversationHistory, ConversationMeta, ConversationPlan, ConversationWords, JobFacts, JobOutput, Kind, Timeline,
    };

    /// A kind's number is its place in the registry, which only grows.
    #[test]
    fn kinds_are_numbered_as_registered() {
        let place = |id: &str| fastctx_sync::registry::REGISTRY.entries().iter().position(|kind| kind.id == id);
        assert_eq!(place("conversation.timeline"), Some(usize::from(Timeline::KIND)));
        assert_eq!(place("conversation.metadata"), Some(usize::from(ConversationMeta::KIND)));
        assert_eq!(place("conversation.words"), Some(usize::from(ConversationWords::KIND)));
        assert_eq!(place("conversation.attachment"), Some(usize::from(ConversationAttachment::KIND)));
        assert_eq!(place("job.facts"), Some(usize::from(JobFacts::KIND)));
        assert_eq!(place("job.output"), Some(usize::from(JobOutput::KIND)));
        assert_eq!(place("conversation.plan"), Some(usize::from(ConversationPlan::KIND)));
        assert_eq!(place("conversation.history"), Some(usize::from(ConversationHistory::KIND)));
    }
}
