//! Late structured failure reports augment evidence without rewriting a settled outcome.

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, TurnEnd};
use serde_json::Value;
use std::io;

fn path(node: &Node, resident: &ResidentId, turn: &str) -> std::path::PathBuf {
    node.world
        .conversation(resident)
        .books()
        .join("turn-failures")
        .join(format!("{}.json", fastctx_hands::files::content_hash(turn.as_bytes())))
}

pub(super) async fn observe(node: &Node, resident: &ResidentId, event: SessionEvent) -> io::Result<SessionEvent> {
    let Happened::Changed { turn: Some(turn), change: SessionChange::Metadata { metadata } } = &event.happened else {
        return Ok(event);
    };
    let Some(failure) = metadata.get("fastctx_turn_failure") else { return Ok(event) };
    if failure["turn"].as_str() != Some(turn) {
        return Err(io::Error::other("failure report belongs to another turn"));
    }
    let path = path(node, resident, turn);
    tokio::task::spawn_blocking(move || {
        let Happened::Changed { change: SessionChange::Metadata { metadata }, .. } = &event.happened else {
            unreachable!()
        };
        let entry = crate::records::metadata::prepare(&path)?;
        let bytes = serde_json::to_vec(&metadata["fastctx_turn_failure"])?;
        crate::records::write_at(&entry.directory, fastctx_storage::Priority::Normal, bytes.len(), move |rows| {
            rows.put_raw(&entry.family, "value", &bytes)
        })?;
        Ok(event)
    })
    .await
    .map_err(io::Error::other)?
}

/// Host diagnostics supplement a World-observed exit; they cannot replace its stop provenance.
pub(super) fn augment(end: &mut TurnEnd, error: Value) {
    if let Some(recorded) = end.error.as_mut().and_then(Value::as_object_mut)
        && recorded.contains_key("world")
    {
        recorded.insert("host_diagnostic".into(), error);
        return;
    }
    end.reason = error["message"].as_str().map(str::to_owned).or(end.reason.take());
    end.error = Some(error);
}

pub(super) fn get(node: &Node, resident: &ResidentId, turn: &str) -> io::Result<Option<Value>> {
    crate::records::metadata::get::<Value>(&path(node, resident, turn))?
        .map(|failure| {
            if failure["turn"].as_str() != Some(turn) {
                return Err(io::Error::other("failure report belongs to another turn"));
            }
            Ok(failure.get("error").cloned())
        })
        .transpose()
        .map(Option::flatten)
}
