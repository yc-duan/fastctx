//! A durable reference to a letter queued locally, distinct from the hub's
//! later acceptance. It survives reconnects and either process restarting.

use std::io;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use serde::{Deserialize, Serialize};

use super::{Inner, Link};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Posted {
    pub(super) epoch: u64,
    pub(super) seq: u64,
}

pub enum Posting {
    Pending,
    Held,
    /// The durable outbox was replaced; the business letter must be reposted.
    Replaced,
    Refused(String),
}

fn refused_path(inner: &Inner, posted: Posted) -> std::path::PathBuf {
    inner.dir.path().join("posting-refusals").join(format!("{}-{}.json", posted.epoch, posted.seq))
}

pub(super) fn refused(inner: &Inner, seq: u64, reason: &str) -> io::Result<()> {
    let epoch = inner.letters.lock().expect("never poisoned").file.epoch;
    let path = refused_path(inner, Posted { epoch, seq });
    ensure_private_dir(path.parent().expect("posting refusals directory"))?;
    let bytes = serde_json::to_vec(&(jiff::Timestamp::now(), reason)).map_err(io::Error::other)?;
    publish(&path, &bytes, Permissions::Private)
}

impl Link {
    /// The hub's persisted outcome for a locally posted letter. A refusal
    /// never becomes success merely because the transport advanced past it.
    pub fn posting(&self, posted: Posted) -> io::Result<Posting> {
        let file = self.inner.letters.lock().expect("never poisoned").file;
        if file.epoch != posted.epoch {
            return Ok(Posting::Replaced);
        }
        let path = refused_path(&self.inner, posted);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let (_, reason): (jiff::Timestamp, String) =
                    serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                return Ok(Posting::Refused(reason));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(if file.held >= posted.seq { Posting::Held } else { Posting::Pending })
    }
}
