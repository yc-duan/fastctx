//! Vouching to a hub for the objects this machine holds of the conversation
//! streams it orders (`fastctx_types::assets::VOUCH`): the hub may let go of
//! its own copies of them, this machine keeping them. Only what is read back
//! whole here — every byte, of its length, hashing to it — is vouched for,
//! with how long ago it was read back: the hub counts it from then. Looked
//! at once in a while, a page of streams at a time and so many bytes read
//! back at most, beside the passes, never holding one up. A stream is told
//! from its start, what is told replacing what was told of it before: again
//! when its objects, its epoch or its holding changed, or once what was told
//! grows old. What is known not to stand any more is told at once — an
//! object not read back whole, a stream none of whose objects is here now,
//! one no longer ordered or held here — so the hub counts it no more. One
//! the hub took not all of, or not all of which was read back yet, is told
//! again at a later look.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Link;
use fastctx_types::access::HubId;
use fastctx_types::assets::{self as wire, Reply, Request};
use sha2::{Digest, Sha256};

use super::session::blocking;

/// How often what this machine holds is looked at for each hub.
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);
/// How many streams one look goes through: most hold no large object, and
/// are passed without a word.
const STREAMS_A_LOOK: usize = 512;
/// The most bytes one look reads back.
const READ_A_LOOK: u64 = 512 << 20;
/// How long what was told of a stream stands before it is told again: well
/// within how long the hub counts what was read back (three days).
const TOLD_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an object read back whole is taken as whole before it is read
/// back again.
const READ_FOR: Duration = Duration::from_secs(2 * 24 * 60 * 60);
/// How long an object found not whole is taken as not whole before it is
/// read back again: read at every look, the ones not whole took the look's
/// reading from every stream after them.
const UNREAD_FOR: Duration = Duration::from_secs(6 * 60 * 60);

/// What was told of a stream: what its objects, epoch and holding came to,
/// when, and whether any object was vouched for.
#[derive(Clone, Copy)]
struct Told {
    digest: [u8; 32],
    at: Instant,
    any: bool,
}

/// What was told each hub, and how far it was looked through.
#[derive(Default)]
pub(crate) struct Vouching {
    /// By hub, scope and stream.
    told: HashMap<(String, String, String), Told>,
    looked: HashMap<String, Instant>,
    /// Where the last look of each hub stopped: the stream gone through last.
    after: HashMap<String, (String, String)>,
    /// The streams each hub's looks went through since they started over.
    passed: HashMap<String, HashSet<(String, String)>>,
    /// The objects read back whole here lately: the life of each read, and
    /// when.
    read: HashMap<[u8; 32], (i64, Instant)>,
    /// The objects found not whole here lately, as `read`.
    unread: HashMap<[u8; 32], (i64, Instant)>,
    /// The hubs a look goes on for now.
    looking: HashSet<String>,
    /// Counted up as what was told a hub is forgotten: a look begun before
    /// keeps nothing it heard.
    forgotten: HashMap<String, u64>,
}

impl Vouching {
    /// What was told `hub` is forgotten: each stream is told it anew, from
    /// its start.
    pub(crate) fn forget(&mut self, hub: &str) {
        self.told.retain(|(told, ..), _| told != hub);
        self.after.remove(hub);
        self.passed.remove(hub);
        *self.forgotten.entry(hub.to_string()).or_default() += 1;
    }
}

pub(crate) type Vouched = Mutex<Vouching>;

/// Looks, beside the pass, at what this machine holds of the conversation
/// streams it orders on `hub`, where it is time to and none goes on now.
pub(super) fn start(node: &Arc<crate::Node>, hub: &HubId, link: &Arc<Link>) {
    let Some(assets) = node.assets.as_ref() else { return };
    if link.asset_version() < wire::VOUCH {
        return;
    }
    {
        let mut vouching = assets.vouched.lock().expect("vouched");
        if vouching.looking.contains(hub.as_str()) || vouching.looked.get(hub.as_str()).is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        vouching.looked.insert(hub.to_string(), Instant::now());
        vouching.looking.insert(hub.to_string());
    }
    let (node, hub, link) = (node.clone(), hub.clone(), link.clone());
    fastctx_platform::task::spawn(async move {
        let told = tell(&node, &hub, &link).await;
        if let Some(assets) = node.assets.as_ref() {
            assets.vouched.lock().expect("vouched").looking.remove(hub.as_str());
        }
        if let Err(why) = told {
            node.log.line(format_args!("assets with {hub}: vouching for what this machine holds: {why}"));
        }
    });
}

/// How telling one stream went.
enum Said {
    /// Told whole: every object of it read back and kept by the hub.
    Whole,
    /// Told in part, or not: told again at a later look.
    Part,
    /// The hub is sealed: nothing more is told this look.
    Sealed,
}

/// Tells the hub what this machine holds whole of `stream` of `scope`, in
/// `epoch`, from the stream's start: `whole`, each with when it was read
/// back, replacing whatever was told of it — nothing told withdraws it all.
/// Nothing is asked once `still` says the look no longer stands: the hub
/// was put back since it began, or is one that takes no vouching now.
async fn say(
    link: &Arc<Link>,
    still: &(dyn Fn() -> bool + Sync),
    (scope, stream, epoch): (&str, &str, u64),
    whole: &[([u8; 32], Instant)],
    told: &mut usize,
    failed: &mut Option<String>,
) -> Said {
    let batches: Vec<&[([u8; 32], Instant)]> = if whole.is_empty() { vec![&[]] } else { whole.chunks(wire::VOUCHED_A_TIME).collect() };
    let mut said = Said::Whole;
    for (n, batch) in batches.into_iter().enumerate() {
        let read_ago = batch.iter().map(|(_, at)| at.elapsed().as_millis()).max().unwrap_or(0);
        let request = Request::Vouch {
            scope: scope.to_string(),
            stream: stream.to_string(),
            epoch,
            hashes: batch.iter().map(|(hash, _)| wire::Bytes::from(*hash)).collect(),
            read_ago: u64::try_from(read_ago).unwrap_or(u64::MAX),
            anew: n == 0,
        };
        if !still() {
            return Said::Part;
        }
        // Asked as it is: what the hub says of the scope besides is the
        // passes' to act on, each scope under its own turn.
        match link.assets(request).await {
            // Taken in part — the hub not holding yet what refers to the
            // rest — it is told again later.
            Ok(Reply::Vouched { kept }) => {
                *told += kept as usize;
                if (kept as usize) < batch.len() {
                    said = Said::Part;
                }
            }
            Ok(Reply::Revoked { .. }) => return Said::Part,
            Ok(Reply::Sealed { .. }) => return Said::Sealed,
            Ok(_) => {
                failed.get_or_insert(format!("{scope} {stream}: the hub answered something else"));
                return Said::Part;
            }
            Err(why) => {
                failed.get_or_insert(format!("{scope} {stream}: {why}"));
                return Said::Part;
            }
        }
    }
    said
}

/// One look: how many objects were told.
async fn tell(node: &Arc<crate::Node>, hub: &HubId, link: &Arc<Link>) -> Result<usize, String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(0) };
    let (hub_text, device) = (hub.to_string(), assets.device());
    let (after, forgotten) = {
        let mut vouching = assets.vouched.lock().expect("vouched");
        vouching.read.retain(|_, (_, at)| at.elapsed() < READ_FOR);
        vouching.unread.retain(|_, (_, at)| at.elapsed() < UNREAD_FOR);
        let after = vouching.after.get(hub.as_str()).cloned();
        if after.is_none() {
            vouching.passed.remove(hub.as_str());
        }
        (after, vouching.forgotten.get(hub.as_str()).copied().unwrap_or(0))
    };
    // What this look heard is kept only where nothing was forgotten since
    // it began.
    let current = |vouching: &Vouching| vouching.forgotten.get(hub.as_str()).copied().unwrap_or(0) == forgotten;
    let still = || link.asset_version() >= wire::VOUCH && current(&assets.vouched.lock().expect("vouched"));
    let longest = fastctx_types::asset_peers::OBJECT_BYTES;
    let found =
        blocking(&assets.library, move |library| library.vouchable(&hub_text, &device, after, STREAMS_A_LOOK, longest)).await?;
    let to_the_end = found.len() < STREAMS_A_LOOK;
    let (mut told, mut failed, mut budget) = (0, None, READ_A_LOOK);
    let mut through: Option<(String, String)> = None;
    let mut cut = false;
    let mut sealed = false;
    for one in found {
        let key = (hub.to_string(), one.scope.clone(), one.stream.clone());
        {
            let mut vouching = assets.vouched.lock().expect("vouched");
            if current(&vouching) {
                vouching.passed.entry(hub.to_string()).or_default().insert((one.scope.clone(), one.stream.clone()));
            }
        }
        let digest: [u8; 32] = {
            let mut hasher = Sha256::new();
            hasher.update(one.epoch.to_be_bytes());
            hasher.update(one.holding.to_be_bytes());
            for object in &one.objects {
                hasher.update(object.hash);
                hasher.update(object.life.to_be_bytes());
            }
            hasher.finalize().into()
        };
        let before = assets.vouched.lock().expect("vouched").told.get(&key).copied();
        // None of its objects here now: told withdrawn where some were told
        // — or may have been before this node last started, which keeps
        // nothing of what it told.
        if one.objects.is_empty() {
            if before.is_none_or(|told| told.any) {
                match say(link, &still, (&one.scope, &one.stream, one.epoch), &[], &mut told, &mut failed).await {
                    Said::Whole => {
                        let mut vouching = assets.vouched.lock().expect("vouched");
                        if current(&vouching) {
                            vouching.told.insert(key, Told { digest, at: Instant::now(), any: false });
                        }
                    }
                    Said::Part => {}
                    Said::Sealed => {
                        sealed = true;
                        break;
                    }
                }
            }
            through = Some((one.scope.clone(), one.stream.clone()));
            continue;
        }
        if before.is_some_and(|told| told.digest == digest && told.at.elapsed() < TOLD_FOR) {
            through = Some((one.scope.clone(), one.stream.clone()));
            continue;
        }
        // Read back whole lately, in the life of it here now, or now, as far
        // as this look reads: each with when it was read back.
        let mut whole: Vec<([u8; 32], Instant)> = Vec::new();
        let (mut all, mut lost) = (true, false);
        for object in &one.objects {
            let lately = assets
                .vouched
                .lock()
                .expect("vouched")
                .read
                .get(&object.hash)
                .filter(|(life, at)| *life == object.life && at.elapsed() < READ_FOR)
                .map(|(_, at)| *at);
            if let Some(at) = lately {
                whole.push((object.hash, at));
                continue;
            }
            let not_whole = assets
                .vouched
                .lock()
                .expect("vouched")
                .unread
                .get(&object.hash)
                .is_some_and(|(life, at)| *life == object.life && at.elapsed() < UNREAD_FOR);
            if not_whole {
                (all, lost) = (false, true);
                continue;
            }
            // Past what this look reads, the stream waits for the next look,
            // which goes on from it.
            if object.size > budget {
                cut = true;
                break;
            }
            let wanted = object.hash;
            let read_back = blocking(&assets.library, move |library| library.verify_object(&wanted)).await?;
            budget -= object.size;
            if read_back {
                let at = Instant::now();
                assets.vouched.lock().expect("vouched").read.insert(object.hash, (object.life, at));
                whole.push((object.hash, at));
            } else {
                {
                    let mut vouching = assets.vouched.lock().expect("vouched");
                    vouching.read.remove(&object.hash);
                    vouching.unread.insert(object.hash, (object.life, Instant::now()));
                }
                (all, lost) = (false, true);
                failed.get_or_insert(format!("{} {}: an object of it is not here whole, and is not vouched for", one.scope, one.stream));
            }
        }
        // Cut short, it waits for the next look — but for one an object of
        // which was found not whole: what is whole of it so far is told now,
        // the rest no more counted until it is read back.
        if cut && !lost {
            break;
        }
        let said = say(link, &still, (&one.scope, &one.stream, one.epoch), &whole, &mut told, &mut failed).await;
        match said {
            Said::Whole if all && !cut => {
                let mut vouching = assets.vouched.lock().expect("vouched");
                if current(&vouching) {
                    vouching.told.insert(key, Told { digest, at: Instant::now(), any: !whole.is_empty() });
                }
            }
            Said::Sealed => {
                sealed = true;
                break;
            }
            _ => {
                // Something of it is counted at the hub now, whole or not.
                let mut vouching = assets.vouched.lock().expect("vouched");
                if current(&vouching) && !whole.is_empty() {
                    let entry = vouching.told.entry(key).or_insert(Told { digest: [0; 32], at: Instant::now(), any: true });
                    entry.any = true;
                }
            }
        }
        if cut {
            break;
        }
        through = Some((one.scope.clone(), one.stream.clone()));
    }
    // Gone through to the end: what was told of streams no look passed
    // since it started over — no longer ordered or held here — is told
    // withdrawn, as far as the hub takes it, and forgotten here.
    if to_the_end && !cut && !sealed {
        let gone: Vec<(String, String)> = {
            let vouching = assets.vouched.lock().expect("vouched");
            let passed = vouching.passed.get(hub.as_str());
            vouching
                .told
                .iter()
                .filter(|((told_hub, scope, stream), told)| {
                    told_hub == hub.as_str() && told.any && !passed.is_some_and(|passed| passed.contains(&(scope.clone(), stream.clone())))
                })
                .map(|((_, scope, stream), _)| (scope.clone(), stream.clone()))
                .collect()
        };
        for (scope, stream) in gone {
            let epoch = {
                let (scope, stream) = (scope.clone(), stream.clone());
                blocking(&assets.library, move |library| library.known_last_epoch(&scope, &stream)).await.ok().flatten()
            };
            // Forgotten once the hub took the withdrawal, or where no epoch
            // of it is known here to withdraw it in; told withdrawn again at
            // a later round otherwise.
            let withdrawn = match epoch {
                Some(epoch) => {
                    let mut quiet = None;
                    matches!(say(link, &still, (&scope, &stream, epoch), &[], &mut told, &mut quiet).await, Said::Whole)
                }
                None => true,
            };
            let mut vouching = assets.vouched.lock().expect("vouched");
            if withdrawn && current(&vouching) {
                vouching.told.remove(&(hub.to_string(), scope, stream));
            }
        }
    }
    // The next look goes on after the last stream gone through whole, or
    // starts over once all were.
    {
        let mut vouching = assets.vouched.lock().expect("vouched");
        if current(&vouching) {
            match (cut || sealed, to_the_end, through) {
                (false, true, _) => {
                    vouching.after.remove(hub.as_str());
                }
                (_, _, Some(through)) => {
                    vouching.after.insert(hub.to_string(), through);
                }
                (_, _, None) => {}
            }
        }
    }
    failed.map_or(Ok(told), Err)
}
