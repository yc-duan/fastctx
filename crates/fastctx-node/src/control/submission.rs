//! Host controls recorded before waiting for a conversation's admission or host.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::control::{Control, Controlled};
use serde::{Deserialize, Serialize};

use crate::Node;
use crate::submitted::Family;

/// Commands can have been consumed even when their host never returned a receipt.
const SHORT: Family = Family {
    name: "call/control",
    // Most controls wait on their host; unrelated conversations can proceed together.
    at_once: 64,
    // Waking a host and executing a command each have their own two-minute waits.
    within: Duration::from_secs(10 * 60),
    // Control calls have a five-minute caller deadline.
    stays: Duration::from_secs(240),
    again: false,
};

/// Forks create identities, rewinds discard history, and compactions may already run.
const LONG: Family = Family {
    name: "call/control-long",
    // History copies are heavier than ordinary controls and have a separate budget.
    at_once: 8,
    // The control drain already allows 45 minutes, including a 30-minute compaction.
    within: Duration::from_secs(45 * 60),
    stays: Duration::from_secs(240),
    again: false,
};

/// Only original arguments participate in the operation's identity. In particular,
/// a fork's new resident and the control queue's row are created by the work itself.
#[derive(Serialize, Deserialize)]
struct Asked {
    resident: ResidentId,
    control: Control,
}

pub(crate) fn register(node: &Arc<Node>) {
    for family in [SHORT, LONG] {
        crate::submitted::register(node, family, |node: Arc<Node>, asked: Asked| async move {
            // The original action, management and host gates still order controls
            // across families, and with queued words and lifecycle changes.
            super::control_here_now(&node, &asked.resident, asked.control).await
        });
    }
}

/// Answers remain direct human input: their arbitrary JSON may contain
/// credentials and must not be copied into an operation's durable payload.
pub(super) fn takes(control: &Control) -> bool {
    matches!(
        control,
        Control::Command { .. }
            | Control::Compact
            | Control::Model { .. }
            | Control::Rename { .. }
            | Control::Fork { .. }
            | Control::Rewind { .. }
    )
}

fn family(control: &Control) -> &'static Family {
    match control {
        Control::Compact | Control::Fork { .. } | Control::Rewind { .. } => &LONG,
        Control::Command { name, path: None, .. } if matches!(name.as_str(), "compact" | "fork") => &LONG,
        _ => &SHORT,
    }
}

pub(super) async fn here(node: &Arc<Node>, resident: &ResidentId, control: Control) -> Result<Controlled, Failure> {
    let host = super::host_of(node, resident)?;
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    if crate::assists::assisting_known(node, &about.acting)?
        && let Control::Command { name, path, .. } = &control
    {
        let allowed = match host {
            "Claude" => fastctx_hosts::claude::assisted::command_allowed(name),
            "Codex" => fastctx_hosts::codex::assisted::command_allowed(name),
            _ => false,
        };
        if path.is_some() || !allowed {
            return Err(Failure::not_run("This command is unavailable in an assisted conversation."));
        }
    }
    if host == "Codex" {
        crate::logins::workbench::works(node, &about)?;
        crate::codex::control::validate(&control)?;
    } else if host == "Claude" {
        crate::claude::control::validate(&control)?;
    }
    crate::submitted::submit(node, family(&control), resident.as_str(), &Asked { resident: resident.clone(), control })
        .await
}
