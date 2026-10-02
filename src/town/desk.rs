//! A machine's one Computer Use desk (`specs/world-colleagues.md`, "一台机器只有一张桌子").
//!
//! The app's helper takes one request at a time and refuses the rest on the spot without
//! saying who holds it. The World adds what it can see: the first resident whose turn uses
//! Computer Use sits at the desk until that turn ends; a resident refused meanwhile is told
//! who sits there and that it will hear when the desk is free. Nothing is locked: the helper
//! still does the refusing.

use serde_json::Value;
use std::collections::BTreeSet;

/// The MCP server through which a Codex thread uses the app's Windows Computer Use.
const COMPUTER_USE_SERVER: &str = "node_repl";
/// What the app's helper answers a request while another one is in flight.
const HELPER_BUSY: &str = "Computer Use helper already has an active request";

#[derive(Debug, Default)]
pub(crate) struct Desk {
    holder: Option<String>,
    waiting: BTreeSet<String>,
}

/// Words the World owes residents after something happened at the desk.
pub(crate) struct Notice {
    pub(crate) to: String,
    pub(crate) text: String,
}

impl Desk {
    /// A Codex item started. Returns whether `resident` now sits at the desk.
    pub(crate) fn item_started(&mut self, resident: &str, item: &Value) -> bool {
        if self.holder.is_none() && uses_computer(item) {
            self.holder = Some(resident.to_string());
            return true;
        }
        false
    }

    /// A Codex item completed; a refusal by the helper earns the resident a notice.
    pub(crate) fn item_completed(&mut self, resident: &str, item: &Value, machine: &str) -> Option<Notice> {
        if !uses_computer(item) || !item.to_string().contains(HELPER_BUSY) {
            return None;
        }
        match &self.holder {
            Some(holder) if holder == resident => None,
            Some(holder) => self.waiting.insert(resident.to_string()).then(|| Notice {
                to: resident.to_string(),
                text: format!(
                    "=== from the World ===\nComputer Use on {machine} is in use by {holder}; its current turn has been using it. \
It becomes free when that turn ends, and you will be told then. Do not call Computer Use before that notice. If there is \
nothing else to do meanwhile, end your turn now; you will be woken."
                ),
            }),
            None => Some(Notice {
                to: resident.to_string(),
                text: format!(
                    "=== from the World ===\nComputer Use on {machine} is in use by a session outside the World, such as the user's own; \
the World cannot see when it becomes free."
                ),
            }),
        }
    }

    /// A resident's turn ended. When it held the desk, everyone waiting is told it is free.
    /// Returns whether it held the desk, and the notices.
    pub(crate) fn turn_ended(&mut self, resident: &str, machine: &str) -> (bool, Vec<Notice>) {
        if self.holder.as_deref() != Some(resident) {
            return (false, Vec::new());
        }
        self.holder = None;
        let notices = std::mem::take(&mut self.waiting)
            .into_iter()
            .map(|to| Notice { to, text: format!("=== from the World ===\nComputer Use on {machine} is free now. Continue your task.") })
            .collect();
        (true, notices)
    }
}

fn uses_computer(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("mcpToolCall")
        && item.get("server").and_then(Value::as_str) == Some(COMPUTER_USE_SERVER)
}
