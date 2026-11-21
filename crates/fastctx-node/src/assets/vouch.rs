//! Vouching to a hub for the objects this machine holds of the conversation
//! streams it orders (`fastctx_types::assets::VOUCH`): the hub may let go of
//! its own copies of them, this machine keeping them. Only what is read back
//! whole here — every byte, of its length, hashing to it — is vouched for,
//! with how long ago it was read back: the hub counts it from then. Looked
//! at once in a while, a page of streams at a time and so many bytes read
//! back at most, beside the passes, never holding one up. A stream is told
//! from its start, what is told replacing what was told of it before: again
//! when its objects, its epoch or its holding changed, or once what was told
//! grows old — what is not here whole any more goes from what the hub counts
//! with it. One the hub took not all of, or not all of which was read back
//! yet, is told again at a later look.

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
/// How many streams one look goes through.
const STREAMS_A_LOOK: usize = 64;
/// The most bytes one look reads back.
const READ_A_LOOK: u64 = 512 << 20;
/// How long what was told of a stream stands before it is told again: well
/// within how long the hub counts what was read back (three days).
const TOLD_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an object read back whole is taken as whole before it is read
/// back again.
const READ_FOR: Duration = Duration::from_secs(2 * 24 * 60 * 60);

/// What was told each hub, and how far it was looked through.
#[derive(Default)]
pub(crate) struct Vouching {
    /// What each stream's objects, epoch and holding came to as they were
    /// told, and when, by hub, scope and stream.
    told: HashMap<(String, String, String), ([u8; 32], Instant)>,
    looked: HashMap<String, Instant>,
    /// Where the last look of each hub stopped: the stream gone through last.
    after: HashMap<String, (String, String)>,
    /// The objects read back whole here lately: the life of each read, and
    /// when.
    read: HashMap<[u8; 32], (i64, Instant)>,
    /// The hubs a look goes on for now.
    looking: HashSet<String>,
}

impl Vouching {
    /// What was told `hub` is forgotten: each stream is told it anew, from
    /// its start.
    pub(crate) fn forget(&mut self, hub: &str) {
        self.told.retain(|(told, ..), _| told != hub);
        self.after.remove(hub);
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

/// One look: how many objects were told.
async fn tell(node: &Arc<crate::Node>, hub: &HubId, link: &Arc<Link>) -> Result<usize, String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(0) };
    let (hub_text, device) = (hub.to_string(), assets.device());
    let after = {
        let mut vouching = assets.vouched.lock().expect("vouched");
        vouching.read.retain(|_, (_, at)| at.elapsed() < READ_FOR);
        vouching.after.get(hub.as_str()).cloned()
    };
    let longest = fastctx_types::asset_peers::OBJECT_BYTES;
    let found =
        blocking(&assets.library, move |library| library.vouchable(&hub_text, &device, after, STREAMS_A_LOOK, longest)).await?;
    let to_the_end = found.len() < STREAMS_A_LOOK;
    let (mut told, mut failed, mut budget) = (0, None, READ_A_LOOK);
    let mut through: Option<(String, String)> = None;
    let mut cut = false;
    for one in found {
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
        let key = (hub.to_string(), one.scope.clone(), one.stream.clone());
        let fresh =
            assets.vouched.lock().expect("vouched").told.get(&key).is_some_and(|(said, at)| *said == digest && at.elapsed() < TOLD_FOR);
        if fresh {
            through = Some((one.scope.clone(), one.stream.clone()));
            continue;
        }
        // Read back whole lately, in the life of it here now, or now, as far
        // as this look reads: each with when it was read back.
        let mut whole: Vec<([u8; 32], Instant)> = Vec::new();
        let mut all = true;
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
                assets.vouched.lock().expect("vouched").read.remove(&object.hash);
                all = false;
                failed.get_or_insert(format!("{} {}: an object of it is not here whole, and is not vouched for", one.scope, one.stream));
            }
        }
        if cut {
            break;
        }
        // Told from its start: the first batch sets aside what was told of it
        // before, so what is not here whole any more goes with it — told
        // even where nothing of it is whole.
        let batches: Vec<&[([u8; 32], Instant)]> = if whole.is_empty() { vec![&[]] } else { whole.chunks(wire::VOUCHED_A_TIME).collect() };
        for (n, batch) in batches.into_iter().enumerate() {
            let read_ago = batch.iter().map(|(_, at)| at.elapsed().as_millis()).max().unwrap_or(0);
            let request = Request::Vouch {
                scope: one.scope.clone(),
                stream: one.stream.clone(),
                epoch: one.epoch,
                hashes: batch.iter().map(|(hash, _)| wire::Bytes::from(*hash)).collect(),
                read_ago: u64::try_from(read_ago).unwrap_or(u64::MAX),
                anew: n == 0,
            };
            // Asked as it is: what the hub says of the scope besides is the
            // passes' to act on, each scope under its own turn.
            match link.assets(request).await {
                // Taken in part — the hub not holding yet what refers to the
                // rest — it is told again later.
                Ok(Reply::Vouched { kept }) => {
                    told += kept as usize;
                    if (kept as usize) < batch.len() {
                        all = false;
                    }
                }
                Ok(Reply::Revoked { .. }) => {
                    all = false;
                    break;
                }
                Ok(Reply::Sealed { .. }) => {
                    all = false;
                    cut = true;
                    break;
                }
                Ok(_) => {
                    all = false;
                    failed.get_or_insert(format!("{} {}: the hub answered something else", one.scope, one.stream));
                    break;
                }
                Err(why) => {
                    all = false;
                    failed.get_or_insert(format!("{} {}: {why}", one.scope, one.stream));
                    break;
                }
            }
        }
        if all {
            assets.vouched.lock().expect("vouched").told.insert(key, (digest, Instant::now()));
        }
        if cut {
            break;
        }
        through = Some((one.scope.clone(), one.stream.clone()));
    }
    // The next look goes on after the last stream gone through whole, or
    // starts over once all were.
    {
        let mut vouching = assets.vouched.lock().expect("vouched");
        match (cut, to_the_end, through) {
            (false, true, _) => {
                vouching.after.remove(hub.as_str());
            }
            (_, _, Some(through)) => {
                vouching.after.insert(hub.to_string(), through);
            }
            (_, _, None) => {}
        }
    }
    failed.map_or(Ok(told), Err)
}
