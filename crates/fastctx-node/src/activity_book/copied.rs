//! Another machine's activity, from the copies here (plan 3-5, design
//! §28.27, §28.58). Each account's records' scope holds what each machine
//! did for it as `activity/<machine>` (releases before beta.26), with the hours an
//! earlier version kept without their deeds; a body's `records` scope what a
//! machine shared whole did for its members. Its book is answered from them
//! as it would answer the accounts asking — what it did for them, and for
//! its own user where one of them owns it — only while the machine is away
//! or does not answer (`query`): since beta.26 no machine writes there.

use crate::Node;
use fastctx_content::v2::{AccountActivity, Entry, Kind as _, Library};
use fastctx_types::access::{Acting, Written};
use fastctx_types::activity_book::{BookAnswer, BookQuery, Deed, Hour, Outcome, Touched};
use std::collections::{BTreeMap, BTreeSet};

/// The most deeds one answer carries, as the machine's own answer.
const MOST: usize = 2000;

/// An hour, in milliseconds.
const HOUR: i64 = 3_600_000;

/// `written`'s book as the copies here have it for `acting`; none when its
/// name alone could be two machines of two networks, a deed's body is not
/// all here, or no copy here has anything of it.
pub(crate) fn answer(node: &Node, acting: &Acting, written: &Written, query: &BookQuery) -> Option<BookAnswer> {
    let machine = &written.name;
    let library = &node.assets.as_ref()?.library;
    // Its stream on the network it is named on, or on each that lists it
    // as the same machine, and whose the accounts asking are.
    let (mut streams, mut logins, mut keys) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    // The scopes of the asking accounts' own records, by key.
    let mut own = BTreeSet::new();
    for link in node.links() {
        if written.hub.as_ref().is_some_and(|hub| link.hub_name() != *hub) {
            continue;
        }
        let view = link.view();
        let Some(member) = view.member(machine) else { continue };
        let accounts = node.accounts_on(&link, acting);
        for account in &accounts {
            if let Some(found) = view.directory.account(account) {
                own.insert(fastctx_types::assets::records_key(
                    link.membership().id.as_str(),
                    account,
                    found.opened.as_millisecond(),
                ));
            }
        }
        let hub = link.membership().id.to_string();
        logins.extend(accounts.iter().map(|account| format!("{hub}/{account}")));
        let named: String = member.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        keys.insert(named.clone());
        streams.insert((hub, format!("{}/{named}", AccountActivity::STREAM)));
    }
    if keys.len() > 1 {
        return None;
    }
    // The window asked: what lies outside it is passed over as it is read.
    let (from, to) = match query {
        BookQuery::Deeds { from, to, .. } | BookQuery::Hours { from, to } => (*from, *to),
    };
    let mut deeds: BTreeMap<String, Deed> = BTreeMap::new();
    let mut legacy: Vec<Hour> = Vec::new();
    let mut found = false;
    let mut waiting = false;
    for scope in library.held_scopes().ok()? {
        // An asking account's own records not pulled whole yet: what the
        // others hold is not all of it.
        if crate::timeline::copy::records_coming(node, &scope) {
            waiting |= own.contains(&scope.key);
            continue;
        }
        if !crate::timeline::copy::records_here(node, &scope) {
            continue;
        }
        // A body's records hold its members' deeds alike, every account's
        // and the machine's own user's: a member reads them all, each with
        // its account, as the machine shows them to whoever may work on it
        // (§29 2.5, `net-shared-machine-visibility` 2026-11-10). Only a
        // member holds them here (`records_here`).
        for (_, stream) in streams.iter().filter(|(hub, _)| scope.hub.as_deref() == Some(hub.as_str())) {
            // Taken only once read: asked for now where it is not here, and
            // no whole answer till it is.
            if !crate::assets::lazy::held(node, &scope, stream) {
                waiting = true;
                continue;
            }
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
                    if value.get("hour").and_then(serde_json::Value::as_bool) == Some(true) {
                        legacy.extend(hour_of(&value).filter(|hour| hour.at >= from && hour.at < to));
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
    if !found || waiting {
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
                let hour = hours
                    .entry((at, deed.session.clone().unwrap_or_default(), agent, deed.held))
                    .or_insert_with(|| Hour {
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
            let mut hours: Vec<Hour> = hours.into_values().collect();
            hours.extend(legacy);
            hours.sort_by_key(|hour| hour.at);
            BookAnswer::Hours { machine: machine.clone(), hours }
        }
    })
}

/// An hour an earlier version kept without its deeds, as releases before beta.26
/// writes it.
fn hour_of(value: &serde_json::Value) -> Option<Hour> {
    let text = |name: &str| value.get(name).and_then(serde_json::Value::as_str).filter(|text| !text.is_empty());
    let number = |name: &str| value.get(name).and_then(serde_json::Value::as_i64).unwrap_or(0).max(0);
    Some(Hour {
        at: jiff::Timestamp::from_millisecond(value.get("at")?.as_i64()?).ok()?,
        session: text("session").map(str::to_string),
        agent: text("agent").and_then(|agent| agent.parse().ok()),
        held: value.get("held").and_then(serde_json::Value::as_bool).unwrap_or(false),
        runs: u32::try_from(number("runs")).unwrap_or(u32::MAX),
        failed: u32::try_from(number("failed")).unwrap_or(u32::MAX),
        busy: number("busy") as u64,
        files: u32::try_from(number("files")).unwrap_or(u32::MAX),
    })
}

/// A deed's body as written: its row, or the one object it is in when it
/// was too large to be a body (releases before beta.26, M17).
fn read(library: &Library, entry: &Entry) -> Option<serde_json::Value> {
    serde_json::from_slice(&crate::timeline::copy::whole_body(library, entry)?).ok()
}

/// A deed as the book's row, written as releases before beta.26 writes it, the
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
            .map(|touched| {
                touched.iter().filter_map(|one| serde_json::from_value::<Touched>(one.clone()).ok()).collect()
            })
            .unwrap_or_default(),
        account: text("account").and_then(|key| key.split_once('/')).and_then(|(_, name)| name.parse().ok()),
    })
}
