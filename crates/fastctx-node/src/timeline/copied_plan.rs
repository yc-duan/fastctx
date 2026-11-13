//! A conversation's records the hub orders — its plan, notes, goals and
//! sentinels — as the copy here holds them, read while the hub is not
//! reached (plan 3-5, design §28.27). The hub orders each into the
//! conversation's scope as its account record (`hub assets::mirror`
//! `placed`: the plan as `plan` of the `plan` stream, a note by its name in
//! `history`, `goal` and `goal_past` in `goals`, a sentinel by its id in
//! `sentinels`), so a machine that holds the conversation holds them as the
//! hub last decided; written by the hub alone, they are read here, never
//! written.

use crate::Node;
use fastctx_content::v2::{ConversationPlan, Entry, Kind as _, Library};
use fastctx_types::account_records::Record;
use fastctx_types::plan::{KIND, Page};
use fastctx_types::{MachineName, ResidentId};

/// The plan of the conversation `resident` — the one on `machine`, where
/// given — as the copy a reader here reads holds it; none when no such copy
/// is here, or two are.
pub(crate) fn page(node: &Node, resident: &ResidentId, machine: Option<&MachineName>) -> Option<Page> {
    let record = record(node, resident, machine, ConversationPlan::STREAM, "plan")?
        .filter(|record| record.metadata.kind == KIND && record.metadata.key == resident.as_str());
    let Some(record) = record else { return Some(Page { owner: resident.clone(), metadata: None, body: None }) };
    // Read as the hub's own answer is read (`Link::plan_read`).
    let body = if record.metadata.deleted { None } else { serde_json::from_value(record.body).ok() };
    Some(Page { owner: resident.clone(), metadata: Some(record.metadata), body })
}

/// The account record `item` of `stream` in the copy of the conversation
/// `resident` — the one on `machine`, where given — a reader here reads:
/// none when no such copy is here or two are, `Some(None)` when the copy
/// holds no such record.
pub(crate) fn record(
    node: &Node,
    resident: &ResidentId,
    machine: Option<&MachineName>,
    stream: &str,
    item: &str,
) -> Option<Option<Record>> {
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
    let item = fastctx_content::v2::item_of(item);
    Some(library.item(&scope, stream, &item).ok()?.and_then(|entry| record_of(library, &entry)))
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
