//! What a conversation deleted here left in the asset library.
//!
//! A deletion from before hubs deleted a conversation's scopes with it left
//! the hub describing them, and every machine that held them holding them.
//! Each such conversation — known by its tombstone, and by a scope of it
//! this library still holds — is asked of its hub once more: the hub deletes
//! the scopes and tells the machines holding them, this one among them.
//! While a scope is still held the asking is made again each day, as an
//! asking of that day: a hub that took one and deleted nothing — one of an
//! earlier version, or one that stopped before it did — is asked again.

use super::{Ask, kept_file, read, write};
use crate::Node;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Descriptor, Readers};
use fastctx_types::{Address, ResidentId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the library is looked through at most, and the askings one
/// look keeps: a machine with many such conversations asks of them a few
/// at a time, each asking being work for its hub and every machine of the
/// account.
const EVERY: Duration = Duration::from_secs(10 * 60);
const AT_ONCE: usize = 16;
static LOOKED: Mutex<Option<Instant>> = Mutex::new(None);
/// The scopes asked of, and the day each was: one asked of today waits for
/// tomorrow.
static ASKED: Mutex<BTreeMap<String, i64>> = Mutex::new(BTreeMap::new());
/// Whether the last look left some for a later one.
static MORE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Keeps a deletion to ask of its hub for each conversation deleted here
/// lately of which the library still holds a scope; whether more are left
/// for a later look.
pub(super) async fn look(node: &Arc<Node>) -> bool {
    let Some(assets) = node.assets.as_ref() else { return false };
    {
        let mut looked = LOOKED.lock().expect("never poisoned");
        if looked.is_some_and(|at| at.elapsed() < EVERY) {
            return MORE.load(std::sync::atomic::Ordering::Relaxed);
        }
        *looked = Some(Instant::now());
    }
    MORE.store(false, std::sync::atomic::Ordering::Relaxed);
    let deleted: BTreeSet<ResidentId> = node.residents.tombstones().into_iter().map(|(id, _)| id).collect();
    if deleted.is_empty() {
        return false;
    }
    let library = assets.library.clone();
    let held = match fastctx_platform::task::blocking(move || library.held_scopes()).await {
        Ok(held) => held,
        Err(error) => {
            node.log.line(format_args!("looking for what deleted conversations left in the asset library: {error}"));
            return false;
        }
    };
    let machine = node.machine();
    let today = jiff::Timestamp::now().as_second().div_euclid(24 * 60 * 60);
    let (mut asks, mut more) = (Vec::new(), false);
    for scope in held {
        let Some(id) = scope.key.rsplit('/').next().and_then(|id| id.parse::<ResidentId>().ok()) else { continue };
        if !deleted.contains(&id) || node.residents.about(&id).is_some() {
            continue;
        }
        let Some(described) =
            scope.description.as_deref().and_then(|description| serde_json::from_slice::<Descriptor>(description).ok())
        else {
            continue;
        };
        // One that lived here: born here, or opened here for an account or
        // in a body. Another machine's of the same id is not this one.
        let account = match &described.readers {
            Readers::Conversation { openers, born_on } if born_on.as_ref().is_none_or(|born| node.is_me(born)) => {
                openers.first().cloned()
            }
            Readers::Body { opener, .. } => Some(opener.clone()),
            _ => None,
        };
        let (Some(account), Ok(hub)) = (account, HubId::try_from(described.hub.clone())) else { continue };
        {
            let mut asked = ASKED.lock().expect("never poisoned");
            if asked.get(&scope.key) == Some(&today) {
                continue;
            }
            if asks.len() >= AT_ONCE {
                more = true;
                MORE.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
            asked.insert(scope.key.clone(), today);
        }
        let conversation = Address { machine: machine.clone(), resident: id };
        asks.push(Ask { hub, account, conversation, after: None, round: Some(today) });
    }
    if asks.is_empty() {
        return false;
    }
    let found = asks.len();
    match keep_latest(node, asks).await {
        Ok(()) => node.log.line(format_args!(
            "{found} scope(s) of conversations deleted here are still held; their hubs are asked to delete them"
        )),
        Err(error) => node.log.line(format_args!("keeping what to ask the hubs of deleted conversations' scopes: {error}")),
    }
    more
}

/// Keeps `asks` to be asked of their hubs, each in place of an asking of an
/// earlier day for the same conversation still kept.
async fn keep_latest(node: &Arc<Node>, asks: Vec<Ask>) -> std::io::Result<()> {
    let _file = node.forget_asks.file.lock().await;
    let path = kept_file(node);
    fastctx_platform::task::spawn_blocking(move || {
        let mut kept = read(&path)?;
        for ask in asks {
            kept.retain(|other| {
                other.round.is_none()
                    || other.hub != ask.hub
                    || other.account != ask.account
                    || other.conversation != ask.conversation
            });
            kept.push(ask);
        }
        write(&path, &kept)
    })
    .await
    .map_err(std::io::Error::other)?
}
