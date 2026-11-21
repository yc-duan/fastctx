//! Vouching to a hub for the objects this machine holds of the conversation
//! streams it orders (`fastctx_types::assets::VOUCH`): the hub may let go of
//! its own copies of them, this machine keeping them. Looked at once in a
//! while; each stream's objects told once, again only when they or its epoch
//! changed; one the hub would not take is told again at the next look.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::Link;
use fastctx_types::access::HubId;
use fastctx_types::assets::{self as wire, Reply, Request};
use sha2::{Digest, Sha256};

use super::Assets;
use super::session::{REVOKED, ask, blocking};

/// How often what this machine holds is looked at for each hub.
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);

/// What was told each hub, and when it was last looked at.
#[derive(Default)]
pub(crate) struct Vouching {
    /// What each stream's objects and epoch came to as they were told, by
    /// hub, scope and stream.
    told: HashMap<(String, String, String), [u8; 32]>,
    looked: HashMap<String, Instant>,
}

pub(crate) type Vouched = Mutex<Vouching>;

/// Tells `hub` what this machine holds of the conversation streams it
/// orders there, where it is time to look again: how many objects were told.
pub(super) async fn tell(assets: &Assets, hub: &HubId, link: &Arc<Link>) -> Result<usize, String> {
    if link.asset_version() < wire::VOUCH {
        return Ok(0);
    }
    {
        let mut vouching = assets.vouched.lock().expect("vouched");
        if vouching.looked.get(hub.as_str()).is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return Ok(0);
        }
        vouching.looked.insert(hub.to_string(), Instant::now());
    }
    let (hub_text, device) = (hub.to_string(), assets.device());
    let found = blocking(&assets.library, move |library| library.vouchable(&hub_text, &device)).await?;
    let (mut told, mut failed) = (0, None);
    for one in found {
        let digest: [u8; 32] = {
            let mut hasher = Sha256::new();
            hasher.update(one.epoch.to_be_bytes());
            for hash in &one.hashes {
                hasher.update(hash);
            }
            hasher.finalize().into()
        };
        let key = (hub.to_string(), one.scope.clone(), one.stream.clone());
        if assets.vouched.lock().expect("vouched").told.get(&key) == Some(&digest) {
            continue;
        }
        let mut whole = true;
        for batch in one.hashes.chunks(wire::VOUCHED_A_TIME) {
            let request = Request::Vouch {
                scope: one.scope.clone(),
                stream: one.stream.clone(),
                epoch: one.epoch,
                hashes: batch.iter().map(|hash| wire::Bytes::from(*hash)).collect(),
            };
            match ask(assets, link, request).await {
                Ok(Reply::Vouched { .. }) => told += batch.len(),
                Ok(_) => {
                    whole = false;
                    failed.get_or_insert(format!("{} {}: the hub answered something else", one.scope, one.stream));
                    break;
                }
                Err(why) => {
                    whole = false;
                    if !why.starts_with(REVOKED) {
                        failed.get_or_insert(format!("{} {}: {why}", one.scope, one.stream));
                    }
                    break;
                }
            }
        }
        if whole {
            assets.vouched.lock().expect("vouched").told.insert(key, digest);
        }
    }
    failed.map_or(Ok(told), Err)
}
