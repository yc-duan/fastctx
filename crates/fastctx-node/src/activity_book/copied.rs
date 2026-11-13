//! Another machine's activity, from the copies here while it is away (plan
//! 3-5, design §28.27). Each account's records' scope holds what each
//! machine did for it as `activity/<machine>` (`assets::activity`), a body's
//! `records` scope what a machine shared whole did for its members. While
//! the machine is away from every network both are on, its book is answered
//! from them as it would answer the accounts asking: what it did for them,
//! and for its own user where one of them owns it. Otherwise the machine is
//! asked.

use crate::Node;
use fastctx_content::v2::{AccountActivity, Entry, Kind as _, Library};
use fastctx_types::access::{Acting, Written};
use fastctx_types::activity_book::{BookAnswer, BookQuery, Deed, Hour, Outcome, Touched};
use std::collections::{BTreeMap, BTreeSet};

/// The most deeds one answer carries, as the machine's own answer.
const MOST: usize = 2000;

/// An hour, in milliseconds.
const HOUR: i64 = 3_600_000;

/// `written`'s book as the copies here have it for `acting`, while it is
/// away; none when it is not, its name alone could be two machines of two
/// networks, a deed's body is not all here, or no copy here has anything of
/// it.
pub(crate) fn answer(node: &Node, acting: &Acting, written: &Written, query: &BookQuery) -> Option<BookAnswer> {
    let machine = &written.name;
    if !crate::timeline::copy::away(node, machine) {
        return None;
    }
    let library = &node.assets.as_ref()?.library;
    // Its stream on the network it is named on, or on each that lists it
    // as the same machine; whose the accounts asking are, and whether one
    // of them owns it.
    let (mut streams, mut logins, mut owned, mut keys) = (BTreeSet::new(), BTreeSet::new(), false, BTreeSet::new());
    for link in node.links() {
        if written.hub.as_ref().is_some_and(|hub| link.hub_name() != *hub) {
            continue;
        }
        let view = link.view();
        let Some(member) = view.member(machine) else { continue };
        let accounts = node.accounts_on(&link, acting);
        let hub = link.membership().id.to_string();
        logins.extend(accounts.iter().map(|account| format!("{hub}/{account}")));
        let named: String = member.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        keys.insert(named.clone());
        streams.insert((hub, format!("{}/{named}", AccountActivity::STREAM)));
        owned |= member.home.as_ref().is_some_and(|home| accounts.contains(home));
    }
    if keys.len() > 1 {
        return None;
    }
    // The window asked: what lies outside it is passed over as it is read.
    let (from, to) = match query {
        BookQuery::Deeds { from, to, .. } | BookQuery::Hours { from, to } => (*from, *to),
    };
    let mut deeds: BTreeMap<String, Deed> = BTreeMap::new();
    let mut found = false;
    for scope in library.held_scopes().ok()? {
        if !crate::timeline::copy::records_here(node, &scope) {
            continue;
        }
        // A body's records hold its members' deeds alike: those of the
        // accounts asking, and the machine's own user's where one of them
        // owns it, as the machine itself would show them.
        let body = scope.key.starts_with("b/");
        for (_, stream) in streams.iter().filter(|(hub, _)| scope.hub.as_deref() == Some(hub.as_str())) {
            let mut after = None;
            loop {
                let page = library.page(&scope.key, stream, after.clone(), 500).ok()?;
                let Some(last) = page.last() else { break };
                found = true;
                after = Some((last.order.clone(), last.item.clone()));
                let full = page.len() == 500;
                for entry in page {
                    // A deed whose body is not all here: no whole answer.
                    let value = read(library, &entry)?;
                    let account = value.get("account").and_then(serde_json::Value::as_str).unwrap_or_default();
                    if body && !(logins.contains(account) || (account.is_empty() && owned)) {
                        continue;
                    }
                    let key = value.get("deed").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
                    if let Some(deed) = deed_of(&value).filter(|deed| deed.began >= from && deed.began < to) {
                        deeds.insert(key, deed);
                    }
                }
                if !full {
                    break;
                }
            }
        }
    }
    if !found {
        return None;
    }
    Some(match query {
        BookQuery::Deeds { limit, .. } => {
            let mut list: Vec<Deed> = deeds.into_values().collect();
            list.sort_by(|a, b| b.began.cmp(&a.began));
            let limit = (*limit as usize).min(MOST);
            let more = list.len() > limit;
            list.truncate(limit);
            BookAnswer::Deeds { machine: machine.clone(), deeds: list, more }
        }
        BookQuery::Hours { .. } => {
            let mut hours: BTreeMap<(i64, String, String, bool), Hour> = BTreeMap::new();
            for deed in deeds.values() {
                let began = deed.began.as_millisecond();
                let at = began - began.rem_euclid(HOUR);
                let agent = deed.agent.as_ref().map(|agent| agent.to_string()).unwrap_or_default();
                let hour = hours.entry((at, deed.session.clone().unwrap_or_default(), agent, deed.held)).or_insert_with(|| Hour {
                    at: jiff::Timestamp::from_millisecond(at).unwrap_or_default(),
                    session: deed.session.clone(),
                    agent: deed.agent.clone(),
                    held: deed.held,
                    runs: 0,
                    failed: 0,
                    busy: 0,
                    files: 0,
                });
                hour.runs += 1;
                if matches!(deed.outcome, Outcome::Failed | Outcome::Lost) {
                    hour.failed += 1;
                }
                hour.busy += (deed.ended.unwrap_or(deed.began).as_millisecond() - began).max(0) as u64;
                hour.files += deed.touched.len() as u32;
            }
            BookAnswer::Hours { machine: machine.clone(), hours: hours.into_values().collect() }
        }
    })
}

/// A deed's body as written: its row, or the one object it is in when it
/// was too large to be a body (`assets::activity`, M17).
fn read(library: &Library, entry: &Entry) -> Option<serde_json::Value> {
    serde_json::from_slice(&crate::timeline::copy::whole_body(library, entry)?).ok()
}

/// A deed as the book's row, written as `assets::activity` writes it, the
/// way the book's own answer reads it (`query`).
fn deed_of(value: &serde_json::Value) -> Option<Deed> {
    let text = |name: &str| value.get(name).and_then(serde_json::Value::as_str);
    let number = |name: &str| value.get(name).and_then(serde_json::Value::as_i64);
    let at = |millisecond: i64| jiff::Timestamp::from_millisecond(millisecond).ok();
    Some(Deed {
        job: text("job")?.parse().ok()?,
        began: at(number("began")?)?,
        ended: number("ended").and_then(at),
        from: text("from").and_then(|name| name.parse().ok()),
        session: text("session").map(str::to_string),
        agent: text("agent").and_then(|agent| agent.parse().ok()),
        held: value.get("held").and_then(serde_json::Value::as_bool).unwrap_or(false),
        project: text("project").and_then(|town| town.parse().ok()),
        script: text("script").unwrap_or_default().to_string(),
        outcome: super::query::outcome(text("outcome").unwrap_or_default()),
        code: number("code").and_then(|code| i32::try_from(code).ok()),
        output: number("output").unwrap_or(0).max(0) as u64,
        went: text("went").unwrap_or_default().split_whitespace().filter_map(|name| name.parse().ok()).collect(),
        looked: number("looked").unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
        copied: number("copied").unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
        touched: value
            .get("touched")
            .and_then(serde_json::Value::as_array)
            .map(|touched| touched.iter().filter_map(|one| serde_json::from_value::<Touched>(one.clone()).ok()).collect())
            .unwrap_or_default(),
    })
}
