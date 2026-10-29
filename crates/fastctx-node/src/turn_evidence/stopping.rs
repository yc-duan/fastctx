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

pub(crate) fn remember(node: &Node, resident: &ResidentId, turn: &str, source: StopSource) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    if self::source(node, resident, turn)? == Some(StopSource::User) {
        return Ok(());
    }
    let path = path(node, resident, turn);
    fastctx_platform::ensure_private_dir(path.parent().expect("stop records have a directory"))?;
    fastctx_platform::publish(
        &path,
        &serde_json::to_vec(&Requested { turn: turn.into(), source, at: jiff::Timestamp::now() })?,
        fastctx_platform::Permissions::Private,
    )
}

pub(crate) fn source(node: &Node, resident: &ResidentId, turn: &str) -> io::Result<Option<StopSource>> {
    match std::fs::read(path(node, resident, turn)) {
        Ok(bytes) => {
            let record: Requested = serde_json::from_slice(&bytes)?;
            if record.turn != turn {
                return Err(io::Error::other("stop record belongs to a different turn"));
            }
            Ok(Some(record.source))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
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
