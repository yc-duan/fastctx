//! What this machine hands back to a hub sealed for its recovery (design
//! §28.38 two). The hub was put back from an older copy; what it signed
//! after, this machine may hold of the streams it orders. For each such
//! stream of each scope held of that hub, the hub is asked where it stands;
//! where this machine holds that point and more, its frames from there are
//! handed back one at a time, each with the bytes of the objects it refers
//! to. Where each stream got to is kept under the recovery's count
//! (`handed-back/<recovery>/<scope>/<stream>`): a frame the hub kept is not
//! handed back again for that recovery, one it did not answer is handed
//! back on the next try, and a stream the hub would take no more of is done
//! with. Nothing is set aside meanwhile: the hub's word on where this
//! machine's frames stand comes only once it reopens.

use super::Assets;
use super::session::{ask, blocking};
use fastctx_content::v2::{Header, hub_orders};
use fastctx_net::link::Link;
use fastctx_types::access::HubId;
use fastctx_types::assets::{self as wire, Reply, Request};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The most bytes one frame and its objects are handed back in, as one
/// request goes up in parts: one larger stays evidence here.
const MOST_BYTES: u64 = 10 << 20;
/// How many frames are read at a time.
const AT_ONCE: usize = 16;

/// Where handing back one stream got to.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mark {
    /// The hub kept every frame up to `next`, the last ending with `head`.
    At { next: u64, head: [u8; 32] },
    /// Nothing more of it is handed back for this recovery, and why.
    Done { why: String },
}

fn mark_name(recovery: u64, scope: &str, stream: &str) -> String {
    format!("handed-back/{recovery}/{scope}/{stream}")
}

async fn read_mark(assets: &Assets, name: &str) -> Result<Option<Mark>, String> {
    let name = name.to_string();
    let kept = blocking(&assets.library, move |library| library.checkpoint(&name)).await?;
    Ok(kept.and_then(|bytes| serde_json::from_slice(&bytes).ok()))
}

async fn keep_mark(assets: &Assets, name: &str, mark: &Mark) -> Result<(), String> {
    let (name, bytes) = (name.to_string(), serde_json::to_vec(mark).map_err(|error| error.to_string())?);
    blocking(&assets.library, move |library| library.keep_checkpoint(&name, bytes)).await
}

/// Hands back to the hub `hub`, sealed for the recovery it reopens as
/// `recovery`, what this machine holds of the streams it orders past where
/// it stands. Stops, to go on at the next try, as the hub answers anything
/// but where it stands and what it made of a frame.
pub(super) async fn hand_back(assets: &Assets, hub: &HubId, link: &Arc<Link>, recovery: u64) -> Result<(), String> {
    let key = hub.as_str().to_string();
    let held = blocking(&assets.library, move |library| library.live_scopes_of(&key)).await?;
    for (scope, _, holding) in held {
        let key = scope.clone();
        let standing = blocking(&assets.library, move |library| library.standing(&key)).await?;
        let mut asking = Vec::new();
        let mut from = Vec::new();
        for (stream, sequence, head) in standing.into_iter().filter(|(stream, ..)| hub_orders(stream)) {
            match read_mark(assets, &mark_name(recovery, &scope, &stream)).await? {
                Some(Mark::Done { .. }) => {}
                Some(Mark::At { next, head }) => {
                    if next <= sequence {
                        from.push((stream, next, head));
                    }
                }
                None => asking.push(wire::Cursor { stream, sequence, head: head.into() }),
            }
        }
        if !asking.is_empty() {
            let Reply::Standing { streams } = ask(assets, link, Request::Cursors { scope: scope.clone(), holding, streams: asking }).await? else {
                return Ok(());
            };
            for standing in streams {
                let name = mark_name(recovery, &scope, &standing.stream);
                if standing.agrees {
                    keep_mark(assets, &name, &Mark::Done { why: "the hub holds where this machine stands".into() }).await?;
                    continue;
                }
                let ends: Vec<(u64, [u8; 32])> = standing.ends.iter().filter_map(|(end, head)| Some((*end, head.array()?))).collect();
                // The hub holds nothing of the stream up to where this
                // machine stands: it goes on from the stream's start.
                if ends.is_empty() {
                    from.push((standing.stream, 0, [0; 32]));
                    continue;
                }
                let (key, stream, looked) = (scope.clone(), standing.stream.clone(), ends.clone());
                match blocking(&assets.library, move |library| library.shared_end(&key, &stream, &looked)).await? {
                    Some(end) => {
                        let head = ends.iter().find(|(at, _)| *at == end).map_or([0; 32], |(_, head)| *head);
                        from.push((standing.stream, end + 1, head));
                    }
                    None => {
                        keep_mark(assets, &name, &Mark::Done { why: "what this machine holds forks before the hub's tail".into() }).await?;
                    }
                }
            }
        }
        for (stream, next, head) in from {
            if !stream_back(assets, link, recovery, &scope, &stream, (next, head)).await? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Hands back the frames of `stream` of `scope` from `at` (the next
/// sequence, and the chain value the frame there follows); whether the
/// hub went on answering.
async fn stream_back(assets: &Assets, link: &Arc<Link>, recovery: u64, scope: &str, stream: &str, at: (u64, [u8; 32])) -> Result<bool, String> {
    let name = mark_name(recovery, scope, stream);
    let (mut next, mut previous) = at;
    loop {
        let (key, name_of) = (scope.to_string(), stream.to_string());
        let frames = blocking(&assets.library, move |library| library.export(&key, &name_of, next, AT_ONCE)).await?;
        if frames.is_empty() {
            return Ok(true);
        }
        for frame in frames {
            let header = Header::decode(&frame.header)?;
            if header.first + u64::from(header.count) <= next {
                continue;
            }
            if header.first != next || header.previous != previous {
                keep_mark(assets, &name, &Mark::Done { why: "this machine's frames do not go on from there".into() }).await?;
                return Ok(true);
            }
            let mut bytes = (frame.header.len() + frame.signature.len() + frame.body.len()) as u64;
            let mut objects = Vec::new();
            for &hash in &frame.objects {
                let Some(size) = blocking(&assets.library, move |library| library.size(&hash)).await? else { continue };
                bytes += size;
                if bytes > MOST_BYTES {
                    keep_mark(assets, &name, &Mark::Done { why: "a frame with its objects is larger than one is handed back in".into() }).await?;
                    return Ok(true);
                }
                let read = blocking(&assets.library, move |library| {
                    let mut read = Vec::with_capacity(size as usize);
                    while (read.len() as u64) < size {
                        let piece = library.read(&hash, read.len() as u64, fastctx_content::v2::READ_BYTES)?;
                        if piece.is_empty() {
                            break;
                        }
                        read.extend_from_slice(&piece);
                    }
                    Ok(read)
                })
                .await?;
                objects.push((hash.to_vec().into(), read.into()));
            }
            let request = Request::HandBack {
                scope: scope.to_string(),
                stream: stream.to_string(),
                frame: wire::Frame::whole(frame.header, frame.signature, frame.body),
                objects,
            };
            match ask(assets, link, request).await? {
                Reply::HandedBack { kept: true, .. } => {
                    next = header.first + u64::from(header.count);
                    previous = header.chain();
                    keep_mark(assets, &name, &Mark::At { next, head: previous }).await?;
                }
                Reply::HandedBack { kept: false, why } => {
                    let why = why.unwrap_or_else(|| "the hub did not keep it".into());
                    keep_mark(assets, &name, &Mark::Done { why }).await?;
                    return Ok(true);
                }
                _ => return Ok(false),
            }
        }
    }
}
