//! A conversation's plan as the copy here holds it, read while the hub is
//! not reached (plan 3-5, design §28.27). The hub orders the plan into the
//! conversation's scope as its account record (`hub assets::mirror`), so a
//! machine that holds the conversation holds its plan as the hub last
//! decided it; written by the hub alone, it is read here, never written.

use crate::Node;
use fastctx_content::v2::{ConversationPlan, Entry, Kind as _, Library};
use fastctx_types::account_records::Record;
use fastctx_types::plan::{KIND, Page};
use fastctx_types::{MachineName, ResidentId};

/// The plan of the conversation `resident` — the one on `machine`, where
/// given — as the copy a reader here reads holds it; none when no such copy
/// is here, or two are.
pub(crate) fn page(node: &Node, resident: &ResidentId, machine: Option<&MachineName>) -> Option<Page> {
    let library = &node.assets.as_ref()?.library;
    let mut found = None;
    for scope in library.scopes_ending(&format!("/{resident}")).ok()? {
        if !super::copy::read_here(node, &scope) {
            continue;
        }
        if let Some(machine) = machine
            && !super::copy::lives_on(library, &scope, machine)
        {
            continue;
        }
        if found.replace(scope.key).is_some() {
            return None;
        }
    }
    let scope = found?;
    for entry in library.page(&scope, ConversationPlan::STREAM, None, 16).ok()? {
        let Some(record) = record_of(library, &entry) else { continue };
        if record.metadata.kind != KIND || record.metadata.key != resident.as_str() {
            continue;
        }
        // Read as the hub's own answer is read (`Link::plan_read`).
        let body = if record.metadata.deleted { None } else { serde_json::from_value(record.body).ok() };
        return Some(Page { owner: resident.clone(), metadata: Some(record.metadata), body });
    }
    Some(Page { owner: resident.clone(), metadata: None, body: None })
}

/// The account record an item of the hub's holds: its body, or the one
/// object the body says it is in (AR03).
fn record_of(library: &Library, entry: &Entry) -> Option<Record> {
    let value: serde_json::Value = serde_json::from_slice(&entry.body).ok()?;
    if value.get("in_object").and_then(serde_json::Value::as_bool) != Some(true) {
        return serde_json::from_value(value).ok();
    }
    let [object] = entry.objects.as_slice() else { return None };
    let (mut bytes, mut at) = (Vec::new(), 0u64);
    while at < object.size {
        let piece = library.read(&object.hash, at, 1 << 20).ok()?;
        if piece.is_empty() {
            return None;
        }
        at += piece.len() as u64;
        bytes.extend_from_slice(&piece);
    }
    serde_json::from_slice(&bytes).ok()
}
