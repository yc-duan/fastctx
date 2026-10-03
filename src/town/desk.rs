//! A machine's one Computer Use desk (`specs/world-colleagues.md`, "一台机器只有一张桌子").
//!
//! The app's helper takes one request at a time and refuses the rest on the spot without
//! saying who holds it. The World adds what it can see: the first resident whose Computer Use
//! call goes through sits at the desk until its turn ends; a resident refused meanwhile is told
//! who sits there, or whose call is in flight, and that it will hear when the desk is free. A
//! refusal while no resident sits there and none has a call in flight means a session outside
//! the World holds it. Nothing is locked: the helper still does the refusing.

use serde_json::Value;
use std::collections::BTreeSet;

/// The MCP server through which a Codex thread uses the app's Windows Computer Use.
const COMPUTER_USE_SERVER: &str = "node_repl";
/// What the app's helper answers a request while another one is in flight.
const HELPER_BUSY: &str = "Computer Use helper already has an active request";

#[derive(Debug, Default)]
pub(crate) struct Desk {
    holder: Option<String>,
    /// Residents whose Computer Use call has started and not finished.
    calling: BTreeSet<String>,
    waiting: BTreeSet<String>,
}

/// Words the World owes residents after something happened at the desk.
pub(crate) struct Notice {
    pub(crate) to: String,
    pub(crate) text: String,
}

/// What a finished Computer Use call changed.
pub(crate) enum Change {
    Nothing,
    /// The resident now sits at the desk.
    Sat,
    Notices(Vec<Notice>),
}

impl Desk {
    /// A Codex item started.
    pub(crate) fn item_started(&mut self, resident: &str, item: &Value) {
        if uses_computer(item) {
            self.calling.insert(resident.to_string());
        }
    }

    /// A Codex item completed.
    pub(crate) fn item_completed(&mut self, resident: &str, item: &Value, machine: &str) -> Change {
        if !uses_computer(item) {
            return Change::Nothing;
        }
        self.calling.remove(resident);
        if !item.to_string().contains(HELPER_BUSY) {
            if self.holder.is_none() {
                self.holder = Some(resident.to_string());
                self.waiting.remove(resident);
                return Change::Sat;
            }
            return Change::Nothing;
        }
        // Who is at the desk, as far as the World can see: whoever sits there, else whoever's
        // call is in flight now, which is the one the helper is serving.
        let user = self.holder.clone().or_else(|| self.calling.iter().next().cloned());
        match user {
            Some(user) if user == resident => Change::Nothing,
            Some(user) => {
                if !self.waiting.insert(resident.to_string()) {
                    return Change::Nothing;
                }
                Change::Notices(vec![Notice {
                    to: resident.to_string(),
                    text: format!(
                        "=== from the World ===\nComputer Use on {machine} is in use by {user}; its current turn is using it. \
It becomes free when that turn ends, and you will be told then. Do not call Computer Use before that notice. If there is \
nothing else to do meanwhile, end your turn now; you will be woken."
                    ),
                }])
            }
            None => {
                // Nobody in the World holds it, so whoever waited for a resident to finish will
                // not hear "free" either; they hear the same as this one.
                let outside = format!(
                    "=== from the World ===\nComputer Use on {machine} is in use by a session outside the World, such as the user's own; \
the World cannot see when it becomes free."
                );
                self.waiting.remove(resident);
                let told = std::iter::once(resident.to_string()).chain(std::mem::take(&mut self.waiting));
                Change::Notices(told.map(|to| Notice { to, text: outside.clone() }).collect())
            }
        }
    }

    /// A resident's turn ended. When it held the desk, the desk is free and everyone waiting is
    /// told. Returns whether it held the desk, and the notices.
    pub(crate) fn turn_ended(&mut self, resident: &str, machine: &str) -> (bool, Vec<Notice>) {
        let was_calling = self.calling.remove(resident);
        if self.holder.as_deref() == Some(resident) {
            return (true, self.release(machine));
        }
        // Its call was the one others were told about; with nobody else at the desk, they hear
        // it is free.
        if was_calling && self.holder.is_none() && self.calling.is_empty() {
            return (false, self.release(machine));
        }
        (false, Vec::new())
    }

    /// The app-server stopped and every turn with it: whoever held the desk is gone. Returns
    /// who held it, and the notices for those waiting.
    pub(crate) fn server_gone(&mut self, machine: &str) -> (Option<String>, Vec<Notice>) {
        self.calling.clear();
        let holder = self.holder.clone();
        (holder, self.release(machine))
    }

    fn release(&mut self, machine: &str) -> Vec<Notice> {
        self.holder = None;
        std::mem::take(&mut self.waiting)
            .into_iter()
            .map(|to| Notice { to, text: format!("=== from the World ===\nComputer Use on {machine} is free now. Continue your task.") })
            .collect()
    }
}

fn uses_computer(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("mcpToolCall")
        && item.get("server").and_then(Value::as_str) == Some(COMPUTER_USE_SERVER)
}
