//! A conversation's records the hub orders — its plan, notes, goals and
//! sentinels — as the copy here holds them: a plan and notes read from it
//! first while it is as fresh as anything seen here of them (design §15
//! `switch`, `records_known`), the rest while the hub is not reached (plan
//! 3-5, design §28.27). The hub orders each into the
//! conversation's scope as its account record (`hub assets::mirror`
//! `placed`: the plan as `plan` of the `plan` stream, a note by its name in
//! `history`, `goal` and `goal_past` in `goals`, a sentinel by its id in
//! `sentinels`), so a machine that holds the conversation holds them as the
//! hub last decided; written by the hub alone, they are read here, never
//! written.

use crate::Node;
use fastctx_content::v2::{ConversationPlan, HeldScope, Kind as _, Library};
use fastctx_types::account_records::Record;
use fastctx_types::plan::Page;
use fastctx_types::{MachineName, ResidentId};

/// The plan of the conversation `resident` — the one on `machine` of the
/// hub `hub`, where given — as the copy a reader here reads holds it, shown
/// to `account` of that hub as the hub shows it: which machine wrote it
/// only where the account may work on that machine; none when no such copy
/// is here, or two are.
pub(crate) fn plan(
    node: &Node,
    resident: &ResidentId,
    machine: Option<&MachineName>,
    hub: &str,
    account: &fastctx_types::AccountName,
) -> Option<Page> {
    let library = &node.assets.as_ref()?.library;
    // The copy the account asking reads, not another's logged in here
    // (codex-847f57fd #3).
    let scope = copy_of(node, resident, machine, Some(hub), Some(account))?;
    let record = record_in(library, &scope.key, ConversationPlan::STREAM, "plan")?;
    told(node, hub, account, resident, record)
}

/// `plan`, read first while the hub is reached: none when the copy is not
/// pulled whole, holds no plan, or is behind what was seen here of it —
/// the hub is asked then.
pub(crate) fn fresh_plan(
    node: &Node,
    resident: &ResidentId,
    machine: Option<&MachineName>,
    hub: &str,
    account: &fastctx_types::AccountName,
) -> Option<Page> {
    let scope = copy_of(node, resident, machine, Some(hub), Some(account))?;
    let record = fresh_record(node, &scope, ConversationPlan::STREAM, "plan", fastctx_types::plan::KIND, resident.as_str())?;
    told(node, hub, account, resident, Some(record))
}

/// A plan as the hub tells `account` of it.
fn told(
    node: &Node,
    hub: &str,
    account: &fastctx_types::AccountName,
    resident: &ResidentId,
    record: Option<Record>,
) -> Option<Page> {
    // Read as the hub's own answer is read (`Link::plan_read`), told as it
    // tells it (`hub owned_records::projection`).
    let link = node.links().into_iter().find(|link| link.membership().id.as_str() == hub)?;
    Some(fastctx_client::records_copy::plan_as_told(&link.view().directory, account, resident, record))
}

/// The copy of the conversation `resident` — the one on `machine` of the
/// hub whose id is `hub`, where given — a reader here reads, `reader` where
/// given: none when no such copy is here, or two are.
pub(crate) fn copy_of(
    node: &Node,
    resident: &ResidentId,
    machine: Option<&MachineName>,
    hub: Option<&str>,
    reader: Option<&fastctx_types::AccountName>,
) -> Option<HeldScope> {
    let library = &node.assets.as_ref()?.library;
    let mut found = None;
    for scope in library.scopes_ending(&format!("/{resident}")).ok()? {
        if !super::copy::read_here_as(node, &scope, reader) {
            continue;
        }
        if hub.is_some_and(|hub| scope.hub.as_deref() != Some(hub)) {
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
pub(crate) use fastctx_client::records_copy::record_in;

/// The record `item` of `stream` in the copy `scope`, the account record
/// `kind`/`key`, once it is as fresh as what was seen here of it: waited for
/// a moment while the copy is behind. None — the hub is asked — when the
/// copy is not pulled whole in its holding, does not hold the record (one
/// the hub keeps from before its scope was described may not be in it yet),
/// or stays behind. Call on a thread that may block.
pub(crate) fn fresh_record(
    node: &Node,
    scope: &HeldScope,
    stream: &str,
    item: &str,
    kind: &str,
    key: &str,
) -> Option<Record> {
    if scope.pulled != Some(scope.holding) {
        return None;
    }
    let library = &node.assets.as_ref()?.library;
    let wanted = super::records_known::at_least(kind, key);
    let until = std::time::Instant::now() + super::records_known::CATCHING_UP;
    loop {
        let record = record_in(library, &scope.key, stream, item)??;
        if record.metadata.version >= wanted {
            return Some(record);
        }
        if std::time::Instant::now() >= until {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Every record of `stream` in the copy `scope`, once each record of `kind`
/// seen here under `prefix` is there as fresh as it was seen: none, as
/// `fresh_record`, otherwise. Call on a thread that may block.
pub(crate) fn fresh_records(
    node: &Node,
    scope: &HeldScope,
    stream: &str,
    kind: &str,
    prefix: &str,
) -> Option<Vec<Record>> {
    if scope.pulled != Some(scope.holding) {
        return None;
    }
    let library = &node.assets.as_ref()?.library;
    let wanted = super::records_known::under(kind, prefix);
    let until = std::time::Instant::now() + super::records_known::CATCHING_UP;
    loop {
        let records = records_in(library, &scope.key, stream)?;
        let fresh = wanted.iter().all(|(key, version)| {
            records.iter().any(|record| record.metadata.key == *key && record.metadata.version >= *version)
        });
        if fresh {
            return Some(records);
        }
        if std::time::Instant::now() >= until {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
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
