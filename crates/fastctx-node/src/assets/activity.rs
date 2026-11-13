//! What this machine did for each account, in the account's records' scope
//! (design §28.23, plan 3-4 group 7), a look a minute on each network it is
//! on: each deed of its activity book, as it stands, into the stream
//! `activity/<machine>`, and its agents' usage lines, a piece at a time,
//! into `usage/<machine>` — the streams this machine orders in that scope.
//! A row says whose it is as `<hub>/<account>`; one of no account is the
//! machine's own user's, its owner logged in here. A deed is written again
//! while its journal is still read; a piece of usage once. Where each look
//! stands is kept beside, by network; a scope held anew is written again
//! whole, what is there as it is left alone.

use super::logs::proposed;
use crate::Node;
use fastctx_content::v2::{AccountActivity, AccountUsage, Kind as _, LEASE, Library, Write, Writer};
use fastctx_net::ids::HubId;
use fastctx_storage::rusqlite::params;
use fastctx_types::AccountName;
use fastctx_types::assets::{Holders, Readers};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often what was done here is looked at.
const EVERY: Duration = Duration::from_secs(60);
/// The most deeds, and usage bytes, a look writes; the most a piece holds.
const DEEDS: i64 = 512;
const USAGE_BYTES: u64 = 16 << 20;
const PIECE: u64 = 1 << 20;

/// Starts looking at what was done here every minute.
pub(crate) fn start(node: &Arc<Node>) {
    let weak: Weak<Node> = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let Some(node) = weak.upgrade() else { return };
            if node.assets.is_none() {
                continue;
            }
            for link in node.links() {
                let (working, hub) = (node.clone(), link.hub().clone());
                let view = link.view();
                let Some(me) = view.me() else { continue };
                // The machine's own user: its owner, logged in here.
                let home = me.home.clone().filter(|home| me.logins.contains(home));
                drop(view);
                if let Err(error) = fastctx_platform::task::blocking(move || look(&working, &hub, home.as_ref())).await {
                    node.log.line(format_args!("writing what was done here into the asset library: {error}"));
                }
            }
        }
    });
}

/// The account on `hub` a row naming `account` is of.
fn routed(account: &str, hub: &HubId, home: Option<&AccountName>) -> Option<AccountName> {
    if account.is_empty() {
        return home.cloned();
    }
    let (of, name) = account.split_once('/')?;
    if of != hub.as_str() {
        return None;
    }
    AccountName::try_from(name.to_string()).ok()
}

/// The records' scope of `account` and its holding, when this machine
/// writes `stream` there now; proposed while it is not held.
fn writable(node: &Node, library: &Library, hub: &HubId, account: &AccountName, stream: &str) -> Result<Option<(String, i64)>, String> {
    let Some(scope) = proposed(node, hub, Readers::Account { account: account.clone() }, "records", Holders::Readers)? else {
        return Ok(None);
    };
    let error = |error: fastctx_content::Error| error.to_string();
    let Some(holding) = library.holding(&scope).map_err(error)? else { return Ok(None) };
    let device = node.assets.as_ref().map(|assets| assets.device()).unwrap_or_default();
    if library.orderer(&scope, stream).map_err(error)?.is_some_and(|orderer| orderer != device) {
        return Ok(None);
    }
    Ok(Some((scope, holding)))
}

/// One look at what was done here for the accounts of `hub`.
fn look(node: &Arc<Node>, hub: &HubId, home: Option<&AccountName>) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let machine: String = assets.device()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    deeds(node, hub, home, &format!("{}/{machine}", AccountActivity::STREAM))?;
    usage(node, hub, home, &format!("{}/{machine}", AccountUsage::STREAM))
}

/// Where a look stands, kept beside: by network.
fn standing(library: &Library, name: &str) -> Result<serde_json::Value, String> {
    Ok(library
        .checkpoint(name)
        .map_err(|error| error.to_string())?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({})))
}

/// Whether a scope was held anew since the last look: then everything is
/// looked at again from the start.
fn held_anew(standing: &mut serde_json::Value, scope: &str, holding: i64) -> bool {
    let was = standing["holdings"][scope].as_i64();
    standing["holdings"][scope] = serde_json::json!(holding);
    was.is_some_and(|was| was != holding)
}

/// A deed as the book has it, with the account it is of and whether its
/// journal is read whole.
struct Deed {
    key: String,
    began: i64,
    account: String,
    complete: bool,
    body: serde_json::Value,
}

const DEED_COLUMNS: &str = "d.deed, d.job, d.began, d.ended, d.from_machine, d.session, d.agent, d.held, d.project, d.account, d.script, d.outcome, d.code, d.output, d.went, d.looked, d.copied, EXISTS(SELECT 1 FROM book_complete c WHERE c.deed = d.deed)";

fn deed_of(row: &fastctx_storage::rusqlite::Row<'_>) -> fastctx_storage::rusqlite::Result<Deed> {
    let key: String = row.get(0)?;
    let began: i64 = row.get(2)?;
    let account: String = row.get(9)?;
    let body = serde_json::json!({
        "deed": key,
        "job": row.get::<_, String>(1)?,
        "began": began,
        "ended": row.get::<_, Option<i64>>(3)?,
        "from": row.get::<_, Option<String>>(4)?,
        "session": row.get::<_, Option<String>>(5)?,
        "agent": row.get::<_, Option<String>>(6)?,
        "held": row.get::<_, bool>(7)?,
        "project": row.get::<_, Option<String>>(8)?,
        "script": row.get::<_, String>(10)?,
        "outcome": row.get::<_, String>(11)?,
        "code": row.get::<_, Option<i64>>(12)?,
        "output": row.get::<_, i64>(13)?,
        "went": row.get::<_, String>(14)?,
        "looked": row.get::<_, i64>(15)?,
        "copied": row.get::<_, i64>(16)?,
    });
    Ok(Deed { key, began, account, complete: row.get(17)?, body })
}

/// Writes the deeds since the last look, and those still read, of the
/// accounts of `hub`.
fn deeds(node: &Arc<Node>, hub: &HubId, home: Option<&AccountName>, stream: &str) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    if node.book_database.fault().is_some() {
        return Ok(());
    }
    let name = format!("activity/{hub}");
    let mut at = standing(library, &name)?;
    let (began, after) = (at["began"].as_i64().unwrap_or(i64::MIN), at["deed"].as_str().unwrap_or_default().to_string());
    let open: BTreeSet<String> =
        at["open"].as_array().into_iter().flatten().filter_map(|deed| deed.as_str().map(str::to_string)).collect();
    let (found, touched) = node
        .book_database
        .read_now(move |db| {
            let mut found = Vec::new();
            let mut new = db.prepare_cached(&format!(
                "SELECT {DEED_COLUMNS} FROM book_deeds d WHERE (d.began, d.deed) > (?1, ?2) ORDER BY d.began, d.deed LIMIT ?3"
            ))?;
            for deed in new.query_map(params![began, after, DEEDS], deed_of)? {
                found.push(deed?);
            }
            let mut again = db.prepare_cached(&format!("SELECT {DEED_COLUMNS} FROM book_deeds d WHERE d.deed = ?1"))?;
            for key in &open {
                if found.iter().any(|deed| &deed.key == key) {
                    continue;
                }
                for deed in again.query_map([key], deed_of)? {
                    found.push(deed?);
                }
            }
            let mut touches = db.prepare_cached("SELECT path, how, added, removed FROM book_touches WHERE deed = ?1 ORDER BY path")?;
            let mut touched = BTreeMap::new();
            for deed in &found {
                let rows = touches.query_map([&deed.key], |row| {
                    Ok(serde_json::json!({
                        "path": row.get::<_, String>(0)?,
                        "how": row.get::<_, String>(1)?,
                        "added": row.get::<_, Option<i64>>(2)?,
                        "removed": row.get::<_, Option<i64>>(3)?,
                    }))
                })?;
                touched.insert(deed.key.clone(), rows.collect::<Result<Vec<_>, _>>()?);
            }
            Ok((found, touched))
        })
        .map_err(|error| error.to_string())?;
    let error = |error: fastctx_content::Error| error.to_string();
    let mut writes: BTreeMap<String, (i64, Vec<Write<AccountActivity>>)> = BTreeMap::new();
    let (mut still_open, mut anew) = (BTreeSet::new(), false);
    let (mut last_began, mut last_deed) = (began, at["deed"].as_str().unwrap_or_default().to_string());
    for mut deed in found {
        if deed.began > last_began || (deed.began == last_began && deed.key > last_deed) {
            (last_began, last_deed) = (deed.began, deed.key.clone());
        }
        let Some(account) = routed(&deed.account, hub, home) else { continue };
        let Some((scope, holding)) = writable(node, library, hub, &account, stream)? else {
            // Its scope not held yet: looked at again.
            still_open.insert(deed.key.clone());
            continue;
        };
        anew |= held_anew(&mut at, &scope, holding);
        if !deed.complete {
            still_open.insert(deed.key.clone());
        }
        deed.body["touched"] = serde_json::json!(touched.get(&deed.key).cloned().unwrap_or_default());
        let body = serde_json::to_vec(&deed.body).map_err(|error| error.to_string())?;
        let item = deed.key.into_bytes();
        if library.item(&scope, stream, &item).map_err(error)?.is_some_and(|entry| entry.body == body) {
            continue;
        }
        writes.entry(scope).or_insert_with(|| (holding, Vec::new())).1.push(Write {
            order: item.clone(),
            item,
            value: Some(body),
            objects: Vec::new(),
        });
    }
    for (scope, (holding, writes)) in writes {
        Writer::<AccountActivity>::new(library, &scope, stream)
            .map_err(error)?
            .in_holding(holding)
            .write_all(writes, Vec::new(), false, assets.signer())
            .map_err(error)?;
    }
    if anew {
        (last_began, last_deed) = (i64::MIN, String::new());
    }
    at["began"] = serde_json::json!(last_began);
    at["deed"] = serde_json::json!(last_deed);
    at["open"] = serde_json::json!(still_open);
    library.keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)
}

/// Writes the usage lines since the last look of the accounts of `hub`,
/// each account's lines of a piece as one object.
fn usage(node: &Arc<Node>, hub: &HubId, home: Option<&AccountName>, stream: &str) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    let root = node.world.path().join("usage");
    let ledgers = match fastctx_platform::disk::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("looking at {}: {error}", root.display())),
    };
    let mut files = Vec::new();
    for ledger in ledgers.flatten() {
        let Some(dir) = ledger.file_name().to_str().map(str::to_string) else { continue };
        let Ok(days) = fastctx_platform::disk::read_dir(ledger.path()) else { continue };
        for day in days.flatten() {
            let Some(date) = day.file_name().to_str().and_then(|name| name.strip_suffix(".jsonl")).map(str::to_string) else {
                continue;
            };
            files.push((format!("{dir}/{date}"), day.path()));
        }
    }
    files.sort();
    let name = format!("usage/{hub}");
    let mut at = standing(library, &name)?;
    let error = |error: fastctx_content::Error| error.to_string();
    let (mut budget, mut anew) = (USAGE_BYTES, false);
    for (file, path) in files {
        if budget == 0 {
            break;
        }
        let offset = at["files"][&file].as_u64().unwrap_or(0);
        let length = match fastctx_platform::disk::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(_) => continue,
        };
        if length <= offset {
            continue;
        }
        let bytes = {
            use std::io::{Read as _, Seek as _, SeekFrom};
            let mut opened = fastctx_platform::disk::open(&path).map_err(|error| error.to_string())?;
            opened.seek(SeekFrom::Start(offset)).map_err(|error| error.to_string())?;
            let mut bytes = Vec::new();
            opened.take(PIECE.min(budget)).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
            bytes
        };
        // Whole lines only: the rest is read when it is whole.
        let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') else { continue };
        let bytes = &bytes[..=end];
        let mut lines: BTreeMap<AccountName, Vec<u8>> = BTreeMap::new();
        for line in bytes.split_inclusive(|&byte| byte == b'\n') {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else { continue };
            let account = match value.get("who").filter(|who| !who.is_null()) {
                Some(who) => {
                    let (Some(of), Some(name)) = (who["hub"].as_str(), who["account"].as_str()) else { continue };
                    routed(&format!("{of}/{name}"), hub, home)
                }
                None => home.cloned(),
            };
            if let Some(account) = account {
                lines.entry(account).or_default().extend_from_slice(line);
            }
        }
        let mut waiting = false;
        for (account, lines) in lines {
            let Some((scope, holding)) = writable(node, library, hub, &account, stream)? else {
                waiting = true;
                continue;
            };
            anew |= held_anew(&mut at, &scope, holding);
            let item = format!("{file}/{offset:016x}").into_bytes();
            if library.item(&scope, stream, &item).map_err(error)?.is_some() {
                continue;
            }
            let lease = library.put(&mut lines.as_slice(), "usage", LEASE).map_err(error)?;
            let body = serde_json::to_vec(&serde_json::json!({ "file": file, "offset": offset, "length": lines.len() }))
                .map_err(|error| error.to_string())?;
            Writer::<AccountUsage>::new(library, &scope, stream)
                .map_err(error)?
                .in_holding(holding)
                .write_all(vec![Write { order: item.clone(), item, value: Some(body), objects: vec![lease.object] }], vec![lease], false, assets.signer())
                .map_err(error)?;
        }
        // A scope not held yet: this piece is read again next time.
        if waiting {
            continue;
        }
        at["files"][&file] = serde_json::json!(offset + bytes.len() as u64);
        budget = budget.saturating_sub(bytes.len() as u64);
    }
    if anew {
        at["files"] = serde_json::json!({});
    }
    library.keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)
}
