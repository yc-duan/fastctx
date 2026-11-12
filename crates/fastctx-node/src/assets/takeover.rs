//! A stream another device ordered, taken over by this one (design §8):
//! a conversation came to live here, and its timeline is ordered here from
//! now on. Asked for by what would write to the stream (`want`), done by the
//! hub's session once this machine holds what the hub has of the stream
//! (`run`): the hub hands it over at the end of what is here, and the new
//! epoch it signs is where this machine's writes go on.
//!
//! The device it leaves orders it no further than that cut. What it wrote
//! past what reached the hub is not carried: a timeline is made again from
//! its conversation's history, which came here with it, and the first
//! reading here writes whatever of it the hub lacks.

use super::session::{ask, blocking};
use super::{Assets, Pinning};
use crate::Node;
use fastctx_content::v2::{EpochFact, HubVerifier as _, Signed};
use fastctx_net::link::Link;
use fastctx_net::sign::PublicKey;
use fastctx_types::access::HubId;
use fastctx_types::assets::{Reply, Request};
use std::sync::Arc;

/// Asks for `stream` of `scope` with `hub` to be ordered here from now on.
pub(crate) fn want(node: &Arc<Node>, hub: &HubId, scope: &str, stream: &str) {
    let Some(assets) = node.assets.as_ref() else { return };
    let added = assets.takeovers.lock().expect("takeovers").insert((scope.to_string(), stream.to_string()));
    if added {
        node.log.line(format_args!("{scope}/{stream} is to be ordered here: asking {hub} to hand it over"));
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
    // Where the stream stands here: its next sequence and the chain value
    // its last frame leaves.
    let scope_text = scope.to_string();
    let next = blocking(library, move |library| library.streams(&scope_text))
        .await?
        .into_iter()
        .find(|(name, _)| name == stream)
        .map_or(0, |(_, next)| next);
    let scope_text = scope.to_string();
    let head = blocking(library, move |library| library.standing(&scope_text))
        .await?
        .into_iter()
        .find(|(name, sequence, _)| name == stream && sequence + 1 == next)
        .map_or([0u8; 32], |(_, _, head)| head);
    let request = Request::Takeover { scope: scope.to_string(), stream: stream.to_string(), next, head: head.into(), gap: None };
    let Reply::Epoch { descriptor } = ask(assets, link, request).await? else {
        return Err("the hub did not hand the stream over".into());
    };
    let signed = Signed { fact: descriptor.fact.0, signature: descriptor.word.0 };
    if !(Pinning { assets, hub, key: *key }).verify(&signed.fact, &signed.signature) {
        return Err("the descriptor of the epoch handed over is not the hub's".into());
    }
    let fact = EpochFact::decode(&signed.fact)?;
    if fact.scope != scope || fact.stream != stream || fact.device != device || fact.start != next {
        return Err("the hub handed over another epoch than the one asked for".into());
    }
    let (scope_text, stream_text, epoch, descriptor) = (scope.to_string(), stream.to_string(), fact.epoch, signed.fact.clone());
    blocking(library, move |library| {
        library.accept_epoch(&scope_text, signed, &super::session::Trusted)?;
        // Already in it, as a pass that heard of it before has it: nothing
        // moves.
        match library.new_epoch(&scope_text, &stream_text, epoch, descriptor) {
            Err(fastctx_content::Error::Invalid(why)) if why.contains("moves forward") => Ok(()),
            done => done,
        }
    })
    .await
}
