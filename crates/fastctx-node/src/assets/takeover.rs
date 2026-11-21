//! A stream another device ordered, taken over by this one (design §8,
//! §28.10): a conversation came to live here, carrying from the device it
//! left a release of each stream that device ordered, and the frames of it
//! the hub had not taken (`release`). Asked for by what would write to the
//! stream (`want`), done by the hub's session once this machine holds what
//! the hub has of the stream (`run`): the predecessor's frames go to the hub
//! first, then the release; the epoch the hub signs for this machine is
//! taken and opened in one transaction, or, handed on since, taken as
//! history alone.
//!
//! Without a release nothing is asked: holding the stream, this machine is
//! one more replica of it, and what would write to it waits.

use super::session::{ask, blocking};
use super::{Assets, Pinning};
use crate::Node;
use fastctx_content::v2::{EpochFact, Holder, HubVerifier as _, Imported, RawFrame, Signed, Taken};
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Received, Reply, Request};
use std::sync::Arc;

/// The scopes being adopted that looked done, by how many looks running.
static QUIET: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(Default::default);

/// How often the hub may ask again for the objects of one frame.
const OBJECT_ROUNDS: usize = 3;

/// The conversation a scope of a conversation is of: the last part of its
/// key.
fn resident_of(scope: &str) -> &str {
    scope.rsplit('/').next().unwrap_or(scope)
}

/// Asks for `stream` of `scope` with `hub` to be ordered here from now on,
/// when the conversation carries the release for this machine.
pub(crate) fn want(node: &Arc<Node>, hub: &HubId, scope: &str, stream: &str) {
    let Some(assets) = node.assets.as_ref() else { return };
    // Not linked to the hub now, nothing is asked of it.
    if node.link_of(hub).is_none() {
        return;
    }
    let directory = assets.conversation_dir(resident_of(scope));
    let wish = (scope.to_string(), stream.to_string());
    let adopted =
        assets.adopting.lock().expect("adopting").contains(scope) || super::release::adopting(&directory, hub.as_str(), scope, &assets.device());
    if !adopted && super::release::carried(&directory, hub.as_str(), scope, stream, &assets.device()).is_none() {
        // Said once: the producer asks again and again while it waits.
        if assets.unreleased.lock().expect("unreleased").insert(wish) {
            node.log.line(format_args!(
                "{scope}/{stream} is ordered by another device and came with no release for this machine: held as a replica"
            ));
        }
        return;
    }
    if assets.takeovers.lock().expect("takeovers").insert(wish) {
        node.log.line(format_args!("{scope}/{stream} came released to this machine: asking {hub} to hand it over"));
    }
    super::wake(node, hub);
}

/// Takes over what was asked of `scope`, this machine holding what the hub
/// has of it. Asked for a conversation gone from here since, nothing is
/// asked (AP08); taken, what wanted it is made again (AP14).
pub(super) async fn run(
    node: &Arc<Node>,
    assets: &Assets,
    hub: &HubId,
    link: &Arc<Link>,
    key: &PublicKey,
    scope: &str,
) -> Result<(), String> {
    let asked: Vec<String> =
        assets.takeovers.lock().expect("takeovers").iter().filter(|(of, _)| of == scope).map(|(_, stream)| stream.clone()).collect();
    // An adopting not done is looked at again, its streams found since among
    // them (NN11, L05).
    if asked.is_empty() && !assets.adopting.lock().expect("adopting").contains(scope) {
        return Ok(());
    }
    let resident = resident_of(scope);
    let directory = assets.conversation_dir(resident);
    if !fastctx_platform::disk::is_file(directory.join("about")) {
        assets.takeovers.lock().expect("takeovers").retain(|(of, _)| of != scope);
        assets.adopting.lock().expect("adopting").remove(scope);
        super::release::adopted(&directory, &assets.device());
        return Ok(());
    }
    let mut taken = false;
    for stream in asked {
        taken |= take_over(assets, hub, link, key, scope, &stream).await?;
        assets.takeovers.lock().expect("takeovers").remove(&(scope.to_string(), stream));
    }
    // Adopting is over once every stream of the scope is ordered here —
    // those begun later by the device it was taken from among them (MM09,
    // MM15).
    let (scope_text, device) = (scope.to_string(), assets.device());
    let theirs: Vec<String> = blocking(&assets.library, move |library| {
        let own: std::collections::BTreeSet<String> = library.own_streams(&scope_text, &device)?.into_iter().collect();
        Ok(library
            .streams(&scope_text)?
            .into_iter()
            .map(|(stream, _)| stream)
            .filter(|stream| !own.contains(stream) && !fastctx_content::v2::hub_orders(stream))
            .collect())
    })
    .await?;
    // Over only once every stream of the scope is known here — pulled whole
    // since — and none of them is another device's (OO08).
    let (scope_text, pulled) = (scope.to_string(), &assets.library);
    let whole = blocking(pulled, move |library| library.is_pulled(&scope_text)).await?;
    // And so in two looks running, each over the streams as the hub listed
    // them just then: a stream the device it was taken from began as it let
    // the others go is seen by the second (PP06).
    let adopting = assets.adopting.lock().expect("adopting").contains(scope);
    let settled = {
        let mut quiet = QUIET.lock().expect("quiet adoptings");
        // Counted only while adopting: another's leaving it is no look of
        // this one (QQ13).
        if adopting && theirs.is_empty() && whole {
            let looks = quiet.entry(scope.to_string()).or_insert(0);
            *looks += 1;
            *looks >= 2
        } else {
            quiet.remove(scope);
            false
        }
    };
    if settled {
        QUIET.lock().expect("quiet adoptings").remove(scope);
        assets.adopting.lock().expect("adopting").remove(scope);
        super::release::adopted(&directory, &assets.device());
    } else if assets.adopting.lock().expect("adopting").contains(scope) {
        // Streams found since the adopting began are taken over too.
        let mut takeovers = assets.takeovers.lock().expect("takeovers");
        for stream in theirs {
            takeovers.insert((scope.to_string(), stream));
        }
    }
    // A package whose every stream is ordered here now is used up: it goes,
    // and does not travel on with the conversation (J08).
    let device = assets.device();
    let released = super::release::released_here(&directory, scope, &device);
    if !released.is_empty() {
        let mut mine = true;
        for stream in released {
            let (scope_text, library) = (scope.to_string(), &assets.library);
            mine &= blocking(library, move |library| library.orderer(&scope_text, &stream)).await?.is_some_and(|orderer| orderer == device);
        }
        if mine {
            let _ = super::release::clear_package(&directory);
        }
    }
    if taken && let Ok(resident) = fastctx_types::ResidentId::try_from(resident) {
        crate::timeline::described(node, &resident);
    }
    Ok(())
}

/// Takes `stream` of `scope` over; whether it is ordered here now.
async fn take_over(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str) -> Result<bool, String> {
    let library = &assets.library;
    let device = assets.device();
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    let orderer = blocking(library, move |library| library.orderer(&scope_text, &stream_text)).await?;
    if orderer == Some(device) {
        return Ok(false);
    }
    // Gone from here meanwhile, or never released to it: nothing is asked
    // (AG09).
    let directory = assets.conversation_dir(resident_of(scope));
    // Adopted by the account's word, a package beside it — broken, it may
    // be — is not used (LL08).
    let adopted =
        assets.adopting.lock().expect("adopting").contains(scope) || super::release::adopting(&directory, hub.as_str(), scope, &device);
    let carried = if adopted { None } else { super::release::carried(&directory, hub.as_str(), scope, stream, &device) };
    // A stream kept by the machine that orders it (`direct`) is not adopted
    // by the account's word: the hub holds too little of it to say where
    // another machine takes over, and one taken over there would order on
    // from a place the copies are past. Released by its machine, it is
    // taken over as ever, with what that machine sent beside it.
    if adopted && super::direct::stays(link, scope, stream) {
        return Ok(false);
    }
    // Ordered nowhere yet, only the account's adopting begins it here
    // (NN14); otherwise it waits for its first frame where it was born.
    if !adopted && (carried.is_none() || orderer.is_none()) {
        return Ok(false);
    }
    let scope_text = scope.to_string();
    let Some(holding) = blocking(library, move |library| library.holding(&scope_text)).await? else {
        return Ok(false);
    };
    // The objects the predecessor's frames refer to came beside them: held
    // here while they go on to the hub (AP04).
    let mut leases = Vec::new();
    for hash in carried.iter().flat_map(|carried| &carried.objects) {
        let Ok(hash) = <[u8; 32]>::try_from(hash.0.as_slice()) else { continue };
        if blocking(library, move |library| library.size(&hash)).await?.is_some() {
            continue;
        }
        let path = super::release::object_path(&directory, &hash);
        let lease = blocking(library, move |library| {
            let mut file = fastctx_platform::disk::open(&path).map_err(|error| fastctx_content::Error::Invalid(error.to_string()))?;
            library.put(&mut file, "release", fastctx_content::v2::LEASE)
        })
        .await?;
        leases.push(lease);
    }
    // What the predecessor wrote that the hub has not goes to it first:
    // the release reaches no further than what the hub holds. It is held
    // here too, as a replica's: the epoch handed over begins where it ends.
    for frame in carried.iter().flat_map(|carried| &carried.tail) {
        let (mut staged, mut proven, mut rounds) = (0, Vec::new(), 0);
        loop {
            let Reply::Received { received } = super::apart::take(assets, link, scope, frame, &proven, &mut staged).await? else {
                return Err("the hub did not take the predecessor's frame".into());
            };
            let (objects, prove) = match &received {
                Received::Missing { objects } => (objects, false),
                Received::Prove { objects } => (objects, true),
                _ => break,
            };
            rounds += 1;
            if rounds > OBJECT_ROUNDS {
                return Err(format!("{stream} of {scope}: the hub still wants the objects of the predecessor's frames"));
            }
            proven.extend(super::objects::put(library, link, scope, objects, prove).await?);
        }
        let raw = RawFrame {
            header: frame.header.0.clone(),
            signature: frame.signature.0.clone(),
            body: frame.body.0.clone(),
            objects: Vec::new(),
        };
        let scope_text = scope.to_string();
        let imported = blocking(library, move |library| {
            library.import(&scope_text, raw, &super::session::DeviceKeys, Holder::Replica, holding)
        })
        .await?;
        if !matches!(imported, Imported::Applied { .. } | Imported::Already) {
            // Not caught up to where the hub took the predecessor's frames
            // up to: asked again once it is.
            return Err(format!("{stream} of {scope}: the predecessor's frames do not follow what is here ({imported:?})"));
        }
    }
    // Under the release it carried; or, none, as the account that opened it
    // adopts it here.
    let request = Request::Takeover {
        scope: scope.to_string(),
        stream: stream.to_string(),
        release: carried.as_ref().map(|carried| carried.release.clone()),
        adopt: carried.is_none(),
    };
    let pinning = Pinning { assets, hub, key: *key };
    // Gone from here while what came with it went on to the hub: not asked
    // (AQ03, MM11).
    if !fastctx_platform::disk::is_file(directory.join("about")) {
        return Ok(false);
    }
    match ask(assets, link, request).await? {
        Reply::Epoch { descriptor } => {
            let signed = Signed { fact: descriptor.fact.0, signature: descriptor.word.0 };
            if !pinning.verify(&signed.fact, &signed.signature) {
                return Err("the descriptor of the epoch handed over is not the hub's".into());
            }
            let fact = EpochFact::decode(&signed.fact)?;
            if fact.scope != scope || fact.stream != stream || fact.device != device {
                return Err("the hub handed over another epoch than the one asked for".into());
            }
            // Taken and opened in one transaction (AG06); one the hub handed
            // on since is history here (AN02); not before the stream here
            // reaches where it begins, asked again then (AP03).
            let scope_text = scope.to_string();
            let taken = blocking(library, move |library| library.take_over(&scope_text, signed, &super::session::Trusted)).await?;
            drop(leases);
            match taken {
                Taken::Opened => Ok(true),
                Taken::History => Ok(false),
                Taken::Behind => Err(format!("{stream} of {scope} here ends before the epoch handed over begins: it catches up first")),
                Taken::Ahead => Err(format!("{stream} of {scope} here goes on past where the epoch handed over begins: it is not opened")),
                Taken::Forked => Err(format!("{stream} of {scope} here reaches where the epoch handed over begins by another chain: it is not opened")),
            }
        }
        Reply::HandedOn { epochs } => {
            let last = epochs.len().saturating_sub(1);
            let mut opened = false;
            for (index, fact) in epochs.into_iter().enumerate() {
                let signed = Signed { fact: fact.fact.0, signature: fact.word.0 };
                if !pinning.verify(&signed.fact, &signed.signature) {
                    return Err("an epoch's descriptor is not the hub's".into());
                }
                let scope_text = scope.to_string();
                // Handed back to this device since: its own last epoch is
                // opened as a takeover opens it (AQ01).
                if index == last && EpochFact::decode(&signed.fact).is_ok_and(|fact| fact.device == device) {
                    opened |= blocking(library, move |library| library.take_over(&scope_text, signed, &super::session::Trusted)).await?
                        == Taken::Opened;
                    continue;
                }
                blocking(library, move |library| library.accept_epoch(&scope_text, signed, &super::session::Trusted)).await?;
            }
            Ok(opened)
        }
        _ => Err("the hub did not hand the stream over".into()),
    }
}

/// The account that opened `resident`, logged in here, has it written on
/// here from now on (design §28.10): each of its streams another device
/// orders is taken over at the next pass with its hub.
pub(crate) fn adopt(node: &Arc<Node>, resident: &fastctx_types::ResidentId) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Err("the asset library is unavailable".into()) };
    let Some((hub, scope)) = crate::timeline::scope_of(node, resident).map_err(|failure| failure.to_string())? else {
        return Err(format!("{resident} keeps its records on no network here"));
    };
    // Kept beside the conversation first: a node that stops goes on with it
    // when it starts (LL09).
    super::release::adopt(&assets.conversation_dir(resident.as_str()), hub.as_str(), &scope, &assets.device()).map_err(|error| error.to_string())?;
    assets.adopting.lock().expect("adopting").insert(scope.clone());
    assets.unreleased.lock().expect("unreleased").retain(|(of, _)| *of != scope);
    // Every stream of the conversation's scope another device orders now —
    // its timeline, metadata, words, attachments, jobs — and those its
    // producers ask for later (MM15).
    let (library, scope_text, device) = (assets.library.clone(), scope.clone(), assets.device());
    let theirs: Vec<String> = (|| -> Result<Vec<String>, fastctx_content::Error> {
        let own: std::collections::BTreeSet<String> = library.own_streams(&scope_text, &device)?.into_iter().collect();
        Ok(library
            .streams(&scope_text)?
            .into_iter()
            .map(|(stream, _)| stream)
            .filter(|stream| !own.contains(stream) && !fastctx_content::v2::hub_orders(stream))
            .collect())
    })()
    .map_err(|error| error.to_string())?;
    for stream in theirs.iter().map(String::as_str).chain([<fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM]) {
        want(node, &hub, &scope, stream);
    }
    Ok(())
}

