//! Stop provenance and structured host categories; messages are never classifiers.

use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::session::{TurnEnd, TurnHow};
use fastctx_types::turn_evidence::{StopKind, StopReason, StopSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;

#[derive(Serialize, Deserialize)]
struct Requested {
    turn: String,
    source: StopSource,
    at: jiff::Timestamp,
}

fn path(node: &Node, resident: &ResidentId, turn: &str) -> std::path::PathBuf {
    node.world
        .conversation(resident)
        .books()
        .join("turn-stops")
        .join(format!("{}.json", fastctx_hands::files::content_hash(turn.as_bytes())))
}

pub(crate) async fn remember(
    node: &std::sync::Arc<Node>,
    resident: &ResidentId,
    turn: &str,
    source: StopSource,
) -> io::Result<()> {
    let path = path(node, resident, turn);
    let turn = turn.to_string();
    tokio::task::spawn_blocking(move || {
        let entry = crate::records::metadata::prepare(&path)?;
        crate::records::write_at(&entry.directory, fastctx_storage::Priority::Control, turn.len() + 256, move |rows| {
            if let Some(previous) = rows.get::<Requested>(&entry.family, "value")? {
                if previous.turn != turn {
                    return Err(fastctx_storage::Error::with_kind(
                        fastctx_storage::ErrorKind::Corrupt,
                        "stop record belongs to a different turn",
                    ));
                }
                if previous.source == StopSource::User {
                    return Ok(());
                }
            }
            rows.put(&entry.family, "value", &Requested { turn, source, at: jiff::Timestamp::now() })
        })
    })
    .await
    .map_err(io::Error::other)?
}

pub(crate) fn source(node: &Node, resident: &ResidentId, turn: &str) -> io::Result<Option<StopSource>> {
    let record = crate::records::metadata::get::<Requested>(&path(node, resident, turn))?;
    record
        .map(|record| {
            if record.turn != turn {
                Err(io::Error::other("stop record belongs to a different turn"))
            } else {
                Ok(record.source)
            }
        })
        .transpose()
}

pub(crate) fn classify(end: &TurnEnd, requested: Option<StopSource>, agent: &str) -> StopReason {
    let category = end
        .error
        .as_ref()
        .and_then(|error| {
            error
                .get("world")
                .or_else(|| error.get("codexErrorInfo"))
                .or_else(|| error.pointer("/data/codexErrorInfo"))
                .or_else(|| error.get("category"))
        })
        .cloned();
    let name = category.as_ref().and_then(|value| {
        value
            .as_str()
            .or_else(|| value.as_object().filter(|object| object.len() == 1)?.keys().next().map(String::as_str))
    });
    let kind = match requested {
        Some(StopSource::User) => StopKind::UserStopped,
        Some(StopSource::Allowance) => StopKind::UsageLimited,
        Some(StopSource::Lifecycle) => StopKind::OtherError,
        None => match name {
            Some("host_exited") => StopKind::HostExited,
            Some("node_restarted") => StopKind::NodeRestarted,
            _ if end.how == TurnHow::Interrupted => StopKind::UserStopped,
            Some("invalid_request") if agent == "claude" => StopKind::ContextWindowExceeded,
            Some("contextWindowExceeded" | "prompt_too_long" | "model_context_window_exceeded") => {
                StopKind::ContextWindowExceeded
            }
            Some("usageLimitExceeded" | "rate_limit") => StopKind::UsageLimited,
            _ if end.how == TurnHow::Completed => StopKind::Completed,
            _ => StopKind::OtherError,
        },
    };
    let message = end
        .error
        .as_ref()
        .and_then(|error| error.get("message").and_then(Value::as_str))
        .map(str::to_owned)
        .or_else(|| end.reason.clone());
    StopReason { kind, category, message }
}
