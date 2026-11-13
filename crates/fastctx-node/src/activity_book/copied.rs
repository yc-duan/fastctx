//! Another machine's activity, from the copies here while it is away (plan
//! 3-5, design §28.27). Each account's records' scope holds what each
//! machine did for it as `activity/<machine>` (`assets::activity`), a body's
//! `records` scope what a machine shared whole did for its members. While
//! the machine is away from every network both are on, its book is answered
//! from them as it would answer this computer's own interface: what it did
//! for the accounts logged in here, and for its own user where one of them
//! owns it. Otherwise the machine is asked.

use crate::Node;
use fastctx_content::v2::{AccountActivity, Entry, Kind as _, Library};
use fastctx_types::MachineName;
use fastctx_types::access::Acting;
use fastctx_types::activity_book::{BookAnswer, BookQuery, Deed, Hour, Outcome, Touched};
use std::collections::{BTreeMap, BTreeSet};

/// The most deeds one answer carries, as the machine's own answer.
const MOST: usize = 2000;

/// An hour, in milliseconds.
const HOUR: i64 = 3_600_000;

/// `machine`'s book as the copies here have it, while it is away; none when
/// it is not, or no copy here has anything of it.
pub(crate) fn answer(node: &Node, machine: &MachineName, query: &BookQuery) -> Option<BookAnswer> {
    if !crate::timeline::copy::away(node, machine) {
        return None;
    }
    let library = &node.assets.as_ref()?.library;
    // Its stream by the key it has on each network that lists it, whose the
    // accounts logged in here are, and whether one of them owns it.
    let (mut streams, mut logins, mut owned) = (BTreeSet::new(), BTreeSet::new(), false);
    for link in node.links() {
        let view = link.view();
        let accounts = node.accounts_on(&link, &Acting::Here);
        let hub = link.membership().id.to_string();
        logins.extend(accounts.iter().map(|account| format!("{hub}/{account}")));
        if let Some(member) = view.member(machine) {
            let named: String = member.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
            streams.insert(format!("{}/{named}", AccountActivity::STREAM));
            owned |= member.home.as_ref().is_some_and(|home| accounts.contains(home));
        }
    }
    let mut deeds: BTreeMap<String, Deed> = BTreeMap::new();
    let mut found = false;
    for scope in library.held_scopes().ok()? {
        if !crate::timeline::copy::records_here(node, &scope) {
            continue;
        }
        // A body's records hold its members' deeds alike: those of the
        // accounts logged in here, and the machine's own user's where one of
        // them owns it, as the machine itself would show them.
        let body = scope.key.starts_with("b/");
        for stream in &streams {
            let mut after = None;
            loop {
                let page = library.page(&scope.key, stream, after.clone(), 500).ok()?;
                let Some(last) = page.last() else { break };
                found = true;
                after = Some((last.order.clone(), last.item.clone()));
                let full = page.len() == 500;
                for entry in page {
                    let Some(value) = read(library, &entry) else { continue };
                    let account = value.get("account").and_then(serde_json::Value::as_str).unwrap_or_default();
                    if body && !(logins.contains(account) || (account.is_empty() && owned)) {
                        continue;
                    }
                    let key = value.get("deed").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
                    if let Some(deed) = deed_of(&value) {
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
        BookQuery::Deeds { from, to, limit } => {
            let mut list: Vec<Deed> = deeds.into_values().filter(|deed| deed.began >= *from && deed.began < *to).collect();
            list.sort_by(|a, b| b.began.cmp(&a.began));
            let limit = (*limit as usize).min(MOST);
            let more = list.len() > limit;
            list.truncate(limit);
            BookAnswer::Deeds { machine: machine.clone(), deeds: list, more }
        }
        BookQuery::Hours { from, to } => {
            let mut hours: BTreeMap<(i64, String, String, bool), Hour> = BTreeMap::new();
            for deed in deeds.values().filter(|deed| deed.began >= *from && deed.began < *to) {
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
    let value: serde_json::Value = serde_json::from_slice(&entry.body).ok()?;
    if value.get("in_object").and_then(serde_json::Value::as_bool) != Some(true) {
        return Some(value);
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
