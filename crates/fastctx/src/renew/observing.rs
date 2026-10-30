//! Maintenance may race either side of a handover. A slow reply is retried;
//! one missing reply never turns a successful installation into a failure.

use fastctx_client::renewal::{self, Probe};
use fastctx_types::{ProcessIdentity, layout::WorldDir, renewal::Action};
use std::{future::Future, io, time::Duration};

const LOOK: Duration = Duration::from_secs(30);
const SILENCE: Duration = Duration::from_secs(600);

async fn patient<T, F: Future<Output = io::Result<T>>>(mut call: impl FnMut() -> F) -> io::Result<T> {
    let until = tokio::time::Instant::now() + SILENCE;
    loop {
        let remaining = until.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(LOOK.min(remaining), call()).await {
            Ok(result) => return result,
            Err(_) if tokio::time::Instant::now() < until => {}
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the node answered no maintenance call for ten minutes",
                ));
            }
        }
    }
}

pub(crate) async fn observing(world: &WorldDir) -> io::Result<Probe> {
    match patient(|| renewal::probe(world)).await {
        Err(error) if super::transient(&error) => Ok(Probe::Absent),
        result => result,
    }
}

pub(super) async fn requesting(world: &WorldDir, action: Action) -> io::Result<()> {
    patient(|| renewal::request(world, action.clone())).await
}

pub(super) async fn process_of(world: &WorldDir) -> io::Result<Option<ProcessIdentity>> {
    patient(|| renewal::process(world)).await
}
