//! Late structured failure reports augment evidence without rewriting a settled outcome.

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use serde_json::Value;
use std::io;

fn path(node: &Node, resident: &ResidentId, turn: &str) -> std::path::PathBuf {
    node.world
        .conversation(resident)
        .books()
        .join("turn-failures")
        .join(format!("{}.json", fastctx_hands::files::content_hash(turn.as_bytes())))
}

pub(super) fn observe(node: &Node, resident: &ResidentId, event: &SessionEvent) -> io::Result<()> {
    let Happened::Changed { turn: Some(turn), change: SessionChange::Metadata { metadata } } = &event.happened else {
        return Ok(());
    };
    let Some(failure) = metadata.get("fastctx_turn_failure") else { return Ok(()) };
    if failure["turn"].as_str() != Some(turn) {
        return Err(io::Error::other("failure report belongs to another turn"));
    }
    let path = path(node, resident, turn);
    fastctx_platform::ensure_private_dir(path.parent().expect("failure records have a directory"))?;
    fastctx_platform::publish(&path, &serde_json::to_vec(failure)?, fastctx_platform::Permissions::Private)
}

pub(super) fn get(node: &Node, resident: &ResidentId, turn: &str) -> io::Result<Option<Value>> {
    match std::fs::read(path(node, resident, turn)) {
        Ok(bytes) => {
            let failure: Value = serde_json::from_slice(&bytes)?;
            if failure["turn"].as_str() != Some(turn) {
                return Err(io::Error::other("failure report belongs to another turn"));
            }
            Ok(failure.get("error").cloned())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
