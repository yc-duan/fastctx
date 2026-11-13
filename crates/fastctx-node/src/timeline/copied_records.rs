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
use fastctx_content::v2::{ConversationPlan, HeldScope, Kind as _, Library};
use fastctx_types::account_records::Record;
use fastctx_types::plan::{KIND, Page};
use fastctx_types::{MachineName, ResidentId};

/// The plan of the conversation `resident` — the one on `machine`, where
/// given — as the copy a reader here reads holds it; none when no such copy
/// is here, or two are.
pub(crate) fn plan(node: &Node, resident: &ResidentId, machine: Option<&MachineName>) -> Option<Page> {
    let library = &node.assets.as_ref()?.library;
    let scope = copy_of(node, resident, machine)?;
    let record = record_in(library, &scope.key, ConversationPlan::STREAM, "plan")?
        .filter(|record| record.metadata.kind == KIND && record.metadata.key == resident.as_str());
    let Some(record) = record else { return Some(Page { owner: resident.clone(), metadata: None, body: None }) };
    // Read as the hub's own answer is read (`Link::plan_read`).
    let body = if record.metadata.deleted { None } else { serde_json::from_value(record.body).ok() };
    Some(Page { owner: resident.clone(), metadata: Some(record.metadata), body })
}

/// The copy of the conversation `resident` — the one on `machine`, where
/// given — a reader here reads: none when no such copy is here, or two are.
pub(crate) fn copy_of(node: &Node, resident: &ResidentId, machine: Option<&MachineName>) -> Option<HeldScope> {
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
        if found.replace(scope).is_some() {
            return None;
        }
    }
    found
}

/// The account record `item` of `stream` in the copy `scope`: none when it
/// cannot be read, `Some(None)` when the copy holds no such record.
pub(crate) fn record_in(library: &Library, scope: &str, stream: &str, item: &str) -> Option<Option<Record>> {
    let item = fastctx_content::v2::item_of(item);
    let Some(entry) = library.item(scope, stream, &item).ok()? else { return Some(None) };
    let body = super::copy::whole_body(library, &entry)?;
    Some(Some(serde_json::from_slice(&body).ok()?))
}

/// Every account record of `stream` in the copy `scope`, in the order it
/// holds them; none when one cannot be read.
pub(crate) fn records_in(library: &Library, scope: &str, stream: &str) -> Option<Vec<Record>> {
    let (mut records, mut after) = (Vec::new(), None);
    loop {
        let page = library.page(scope, stream, after.take(), 500).ok()?;
        let full = page.len() == 500;
        for entry in &page {
            let body = super::copy::whole_body(library, entry)?;
            records.push(serde_json::from_slice(&body).ok()?);
        }
        match page.last() {
            Some(last) if full => after = Some((last.order.clone(), last.item.clone())),
            _ => return Some(records),
        }
    }
}
