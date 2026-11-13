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
use fastctx_content::v2::{EpochFact, HubVerifier as _, Signed};
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Received, Reply, Request};
use std::sync::Arc;

/// The conversation a scope of a conversation is of: the last part of its
/// key.
fn resident_of(scope: &str) -> &str {
    scope.rsplit('/').next().unwrap_or(scope)
}

/// Asks for `stream` of `scope` with `hub` to be ordered here from now on,
/// when the conversation carries the release for this machine.
pub(crate) fn want(node: &Arc<Node>, hub: &HubId, scope: &str, stream: &str) {
    let Some(assets) = node.assets.as_ref() else { return };
    let Some(link) = node.link_of(hub) else { return };
    let directory = assets.conversation_dir(resident_of(scope));
    let wish = (scope.to_string(), stream.to_string());
    let adopted = assets.adopting.lock().expect("adopting").contains(scope);
    if !adopted && super::release::carried(&directory, hub.as_str(), scope, stream, link.name().as_str()).is_none() {
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
/// has of it.
pub(super) async fn run(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str) -> Result<(), String> {
    let asked: Vec<String> =
        assets.takeovers.lock().expect("takeovers").iter().filter(|(of, _)| of == scope).map(|(_, stream)| stream.clone()).collect();
    for stream in asked {
        take_over(assets, hub, link, key, scope, &stream).await?;
        assets.takeovers.lock().expect("takeovers").remove(&(scope.to_string(), stream));
    }
    Ok(())
}

async fn take_over(assets: &Assets, hub: &HubId, link: &Arc<Link>, key: &PublicKey, scope: &str, stream: &str) -> Result<(), String> {
    let library = &assets.library;
    let device = assets.device();
    let (scope_text, stream_text) = (scope.to_string(), stream.to_string());
    if blocking(library, move |library| library.orderer(&scope_text, &stream_text)).await?.is_none_or(|orderer| orderer == device) {
        return Ok(());
    }
    // Gone from here meanwhile, or never released to it: nothing is asked
    // (AG09).
    let directory = assets.conversation_dir(resident_of(scope));
    let carried = super::release::carried(&directory, hub.as_str(), scope, stream, link.name().as_str());
    let adopted = assets.adopting.lock().expect("adopting").contains(scope);
    if carried.is_none() && !adopted {
        return Ok(());
    }
    // What the predecessor wrote that the hub has not goes to it first:
    // the release reaches no further than what the hub holds.
    for frame in carried.iter().flat_map(|carried| &carried.tail) {
        let mut staged = 0;
        let Reply::Received { received } = super::apart::take(assets, link, scope, frame, &[], &mut staged).await? else {
            return Err("the hub did not take the predecessor's frame".into());
        };
        if matches!(received, Received::Missing { .. } | Received::Prove { .. }) {
            return Err(format!("{stream} of {scope}: the predecessor's frames refer to objects that did not come here"));
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
            // on since is history here (AN02).
            let scope_text = scope.to_string();
            blocking(library, move |library| library.take_over(&scope_text, signed, &super::session::Trusted)).await?;
            Ok(())
        }
        Reply::HandedOn { epochs } => {
            for fact in epochs {
                let signed = Signed { fact: fact.fact.0, signature: fact.word.0 };
                if !pinning.verify(&signed.fact, &signed.signature) {
                    return Err("an epoch's descriptor is not the hub's".into());
                }
                let scope_text = scope.to_string();
                blocking(library, move |library| library.accept_epoch(&scope_text, signed, &super::session::Trusted)).await?;
            }
            Ok(())
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
    assets.adopting.lock().expect("adopting").insert(scope.clone());
    assets.unreleased.lock().expect("unreleased").retain(|(of, _)| *of != scope);
    // Every stream of a conversation's scope: its timeline and its metadata.
    want(node, &hub, &scope, <fastctx_content::v2::Timeline as fastctx_content::v2::Kind>::STREAM);
    want(node, &hub, &scope, <fastctx_content::v2::ConversationMeta as fastctx_content::v2::Kind>::STREAM);
    Ok(())
}

