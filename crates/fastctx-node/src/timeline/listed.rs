//! The conversations of a machine that is away, as this machine's copies of
//! them have them (plan 3-5, design §28.32). When the hub tells nothing of
//! what was afoot there — it was started again since the machine left — a
//! machine's list is made of what is held here: each conversation's
//! description, project, title, and when it was archived; none of them is
//! here or doing anything. Reads only read.

use crate::Node;
use fastctx_content::v2::{ConversationMeta, Kind as _};
use fastctx_types::MachineName;
use fastctx_types::town::{ResidentNow, WhoReply};

/// The conversations `machine` keeps, as the copies here have them; none
/// when no copy here says it lives there.
pub(crate) fn of(node: &Node, machine: &MachineName) -> Option<WhoReply> {
    let kept = node.assets.as_ref()?;
    let library = &kept.library;
    let mut residents = Vec::new();
    for scope in library.held_scopes().ok()? {
        if !super::copy::read_here(node, &scope) {
            continue;
        }
        let Ok(items) = library.page(&scope.key, ConversationMeta::STREAM, None, 16) else { continue };
        let item = |name: &str| -> Option<serde_json::Value> {
            items.iter().find(|entry| entry.item == name.as_bytes()).and_then(|entry| serde_json::from_slice(&entry.body).ok())
        };
        let (Some(mut about), Some(place)) = (item("about"), item("place")) else { continue };
        if place.get("machine").and_then(|lives| lives.as_str()) != Some(machine.as_str()) {
            continue;
        }
        // Where it lives now, as a list says of each conversation.
        about["machine"] = serde_json::Value::String(machine.to_string());
        let archived = item("archived").and_then(|archived| archived.get("at").cloned());
        let since = archived.clone().or_else(|| about.get("born_at").cloned()).unwrap_or(serde_json::Value::Null);
        let title = item("title").and_then(|title| title.get("title").cloned()).unwrap_or(serde_json::Value::Null);
        let now = serde_json::json!({
            "about": about,
            "town": place.get("project").cloned().unwrap_or(serde_json::Value::Null),
            "here": false,
            "now": { "activity": "idle", "since": since, "turn": null, "answering": [], "trouble": null, "title": title },
            "last_action": null,
            "removed": archived,
        });
        if let Ok(resident) = serde_json::from_value::<ResidentNow>(now) {
            residents.push(resident);
        }
    }
    (!residents.is_empty()).then(|| WhoReply { machine: machine.clone(), residents, jobs: Vec::new(), history: None, busy: None })
}
