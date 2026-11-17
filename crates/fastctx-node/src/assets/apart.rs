//! Frame bodies too large for the control connection (design §7.1): one
//! this machine gives the hub is staged first on a data connection, under a
//! ticket the frame then names; those of a reply of frames come after it,
//! in order, on a data connection under the ticket the reply names.

use super::Assets;
use super::session::ask;
use fastctx_net::link::Link;
use fastctx_types::assets::{self as wire, APART_FRAME_BYTES, Bytes, CHUNK_BYTES, Reply, Request};
use std::sync::Arc;

/// How long frame bodies on a data connection go on before the node says
/// it waits for them (`stuck`).
const APART_WITHIN: std::time::Duration = std::time::Duration::from_secs(300);

/// Asks the hub to take `frame` of `scope`, naming the objects `proven`.
/// A body too large for the control connection is staged first, once:
/// `staged` keeps its ticket for the frame asked again.
pub(super) async fn take(
    assets: &Assets,
    link: &Arc<Link>,
    scope: &str,
    frame: &wire::Frame,
    proven: &[Bytes],
    staged: &mut u64,
) -> Result<Reply, String> {
    // With a hub that keeps asset lines, the frame goes on one, whole
    // however long its body: nothing is staged.
    if link.keeps_asset_lines() {
        let mut received = super::lines::take(assets, link, scope, std::slice::from_ref(frame)).await?;
        if received.len() != 1 {
            return Err("the hub's word on the frame is not one".into());
        }
        return Ok(Reply::Received { received: received.remove(0) });
    }
    if !frame.too_large() {
        let request = Request::Take { scope: scope.to_string(), frame: frame.clone(), proven: proven.to_vec(), staged: 0 };
        return ask(assets, link, request).await;
    }
    if *staged == 0 {
        *staged = stage(assets, link, scope, &frame.body.0).await?;
    }
    let apart = wire::Frame { header: frame.header.clone(), signature: frame.signature.clone(), body: Bytes::default(), apart: frame.body.0.len() as u64 };
    let request = Request::Take { scope: scope.to_string(), frame: apart, proven: proven.to_vec(), staged: *staged };
    let reply = ask(assets, link, request).await;
    if reply.is_err() {
        // The staged body may be gone with what failed: the next ask stages
        // it again.
        *staged = 0;
    }
    reply
}

/// Stages `body` of `scope` on the hub; the ticket it waits under.
async fn stage(assets: &Assets, link: &Arc<Link>, scope: &str, body: &[u8]) -> Result<u64, String> {
    let request = Request::Stage { scope: scope.to_string(), size: body.len() as u64 };
    let Reply::Staged { ticket } = ask(assets, link, request).await? else {
        return Err("the hub gave no room for a frame body".into());
    };
    let _waiting = fastctx_platform::doing::waiting_outside("a frame body going to the hub on a data connection", APART_WITHIN);
    let mut outgoing = link.put_object(ticket).await?;
    for (index, chunk) in body.chunks(CHUNK_BYTES as usize).enumerate() {
        outgoing.chunk(index as u64, chunk).await?;
    }
    outgoing.finish().await?;
    Ok(ticket)
}

/// Brings the bodies of `frames` that travel apart, in order, under
/// `ticket`.
pub(super) async fn bring(link: &Arc<Link>, ticket: u64, frames: &mut [wire::Frame]) -> Result<(), String> {
    if frames.iter().all(|frame| frame.apart == 0) {
        return Ok(());
    }
    if ticket == 0 {
        return Err("the hub sent frame bodies apart under no ticket".into());
    }
    let _waiting = fastctx_platform::doing::waiting_outside("frame bodies coming from the hub on a data connection", APART_WITHIN);
    let mut incoming = link.get_object(ticket).await?;
    let mut expected = 0u64;
    for frame in frames.iter_mut().filter(|frame| frame.apart > 0) {
        if frame.apart > APART_FRAME_BYTES || !frame.body.0.is_empty() {
            return Err("a frame body apart is at most 2 MiB and comes alone".into());
        }
        let mut body = Vec::with_capacity(frame.apart as usize);
        while (body.len() as u64) < frame.apart {
            let (index, bytes) = incoming.chunk().await?;
            if index != expected {
                return Err("frame bodies come in order".into());
            }
            expected += 1;
            body.extend_from_slice(&bytes);
        }
        if body.len() as u64 != frame.apart {
            return Err("a frame body came longer than the hub said".into());
        }
        frame.body = body.into();
        frame.apart = 0;
    }
    incoming.done().await
}
