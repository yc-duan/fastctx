//! The bulk of the asset sync on asset lines (`fastctx_types::asset_lines`),
//! with a hub that keeps them: frames come and go whole, many to an
//! exchange, and objects one after another, on connections that stay open.
//! What a hub that keeps none is asked instead goes as before, on the
//! control connection and under tickets (`session`, `apart`, `objects`).

use super::Assets;
use super::session::heard;
use fastctx_net::link::Link;
use fastctx_types::asset_lines::FRAMES;
use fastctx_types::assets::{self as wire, Reply};
use std::sync::Arc;
use std::time::Duration;

/// How long one exchange on a line goes on before the node says it waits
/// for it: a label, not a bound.
const EXCHANGED_WITHIN: Duration = Duration::from_secs(60);

/// Frames exported at once to go up on a line.
pub(super) const FRAMES_UP_AT_ONCE: usize = 128;

/// How many frames of one stream go up in one turn on lines, and how far
/// along one stream a turn takes it in records: what is left goes on at
/// once, the other streams gone through meanwhile.
pub(super) const FRAMES_UP_A_TURN: usize = 2048;
pub(super) const RECORDS_A_TURN: u64 = 16_384;

/// Up to a line's worth of frames of `stream` of `scope` from the one
/// holding `from`, each whole.
pub(super) async fn frames(
    assets: &Assets,
    link: &Arc<Link>,
    scope: &str,
    stream: &str,
    from: u64,
) -> Result<Vec<wire::Frame>, String> {
    let _waiting =
        fastctx_platform::doing::waiting_outside("frames coming from the hub on an asset line", EXCHANGED_WITHIN);
    let mut line = link.asset_line().await?;
    let reply = line.frames(scope, stream, from, FRAMES as u32).await?;
    drop(line);
    match heard(assets, link, reply).await? {
        Reply::Frames { frames, .. } => Ok(frames),
        _ => Err("the hub did not give frames".into()),
    }
}

/// Gives the hub `frames` of one stream of `scope`, in order, each whole:
/// its word on each, up to the first it neither applied nor parked.
pub(super) async fn take(
    assets: &Assets,
    link: &Arc<Link>,
    scope: &str,
    frames: &[wire::Frame],
) -> Result<Vec<wire::Received>, String> {
    let _waiting =
        fastctx_platform::doing::waiting_outside("frames going to the hub on an asset line", EXCHANGED_WITHIN);
    let mut line = link.asset_line().await?;
    let reply = line.take(scope, frames).await?;
    drop(line);
    match heard(assets, link, reply).await? {
        Reply::ReceivedMany { received } => Ok(received),
        _ => Err("the hub did not take the frames".into()),
    }
}
