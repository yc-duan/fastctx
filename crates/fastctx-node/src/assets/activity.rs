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
//!
//! A machine shared whole holds no account's records' scope, so while it is
//! shared what it did and spent goes into its body's `records` scope instead,
//! read by the body's members (design §29, 2.5). What was done before a share
//! came into force stays its accounts' own: as the machine clears for a share,
//! where the book and each usage file stand is marked, and nothing before the
//! mark goes to the body.

use super::logs::proposed;
use super::usage_marks::{Marked, marked_run, marks_at, marks_through};
use crate::Node;
use fastctx_content::v2::{AccountActivity, AccountUsage, Kind as _, LEASE, Library, Write, Writer};
use fastctx_net::ids::HubId;
use fastctx_storage::rusqlite::params;
use fastctx_types::AccountName;
use fastctx_types::assets::Readers;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// The most a body holds as it is.
const BODY_MOST: usize = fastctx_content::RECORD_BYTES - 4096;

/// How often what was done here is looked at, and how soon after something
/// here changed: copies elsewhere are read first (§28.58), so they keep up.
const EVERY: Duration = Duration::from_secs(60);
const SOON: Duration = Duration::from_secs(5);
/// The most deeds, and usage bytes, a look writes; the most a piece holds.
const DEEDS: i64 = 512;
const USAGE_BYTES: u64 = 16 << 20;
const PIECE: u64 = 1 << 20;

/// Starts looking at what was done here every minute, and a few seconds
/// after anything here changed.
pub(crate) fn start(node: &Arc<Node>) {
    let weak: Weak<Node> = Arc::downgrade(node);
    let mut moves = node.topics.moves();
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::select! {
                () = tokio::time::sleep(EVERY) => {}
                moved = moves.changed() => {
                    if moved.is_err() {
                        return;
                    }
                    tokio::time::sleep(SOON).await;
                    moves.mark_unchanged();
                }
            }
            let Some(node) = weak.upgrade() else { return };
            if node.assets.is_none() {
                continue;
            }
            for link in node.links() {
                let (working, hub) = (node.clone(), link.hub().clone());
                let view = link.view();
                let Some(me) = view.me() else { continue };
                // The machine's own user: its owner, logged in here.
                // A managed machine works for its owner, logged in or not;
                // another's owner logged out for now leaves what is its own
                // user's to wait (OO17).
                let home = me.home.clone().filter(|home| me.managed || me.logins.contains(home));
                let waits = me.home.is_some() && home.is_none();
                let owner = me.home.clone();
                drop(view);
                if let Err(error) =
                    fastctx_platform::task::blocking(move || look(&working, &hub, home.as_ref(), waits, owner.as_ref()))
                        .await
                {
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
pub(crate) fn writable(
    node: &Node,
    library: &Library,
    hub: &HubId,
    account: &AccountName,
    stream: &str,
) -> Result<Option<(String, i64)>, String> {
    let Some(opened) =
        node.link_of(hub).and_then(|link| link.view().directory.account(account).map(|found| found.opened))
    else {
        return Ok(None);
    };
    let name = fastctx_types::assets::records_key("", account, opened.as_millisecond());
    let name = name.rsplit('/').next().unwrap_or_default().to_string();
    let Some(scope) = proposed(node, hub, Readers::Account { account: account.clone() }, &name)? else {
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

/// How far something may seem done before its account was opened, as the
/// hub's clock and this machine's differ, in milliseconds.
const CLOCKS: i64 = 2 * 60 * 1000;

/// When `account` of `hub` was opened, in milliseconds, as the hub this
/// machine is on tells it; none when it is not known here now.
fn opened_ms(node: &Node, hub: &HubId, account: &AccountName) -> Option<i64> {
    node.link_of(hub).and_then(|link| link.view().directory.account(account).map(|found| found.opened.as_millisecond()))
}

/// Whether what was done at `at` (milliseconds) is `account`'s: done before
/// the account of that name was opened, or before this machine first saw it
/// opened so, it was another's of that name, made again since, and goes
/// into none of this one's records (codex-357de52a; `incarnations`).
pub(crate) fn of_this(node: &Node, hub: &HubId, account: &AccountName, at: i64) -> bool {
    opened_ms(node, hub, account).is_none_or(|opened| {
        opened <= at.saturating_add(CLOCKS)
            && node.incarnations.since(node, hub, account, opened).is_none_or(|since| at >= since)
    })
}

/// An hour of activity, as an activity book of before kept it: from its
/// first millisecond, as the book rounds what was done in it down to it.
const HOUR_MS: i64 = 60 * 60 * 1000;

/// The body's `records` scope this machine, shared whole as `body` and owned
/// by `owner`, writes `stream` in now, and its holding; proposed while it is
/// not held.
fn body_scope(
    node: &Node,
    library: &Library,
    hub: &HubId,
    body: &str,
    owner: Option<&AccountName>,
    stream: &str,
) -> Result<Option<(String, i64)>, String> {
    let Some(owner) = owner else { return Ok(None) };
    let readers = Readers::Body { body: body.to_string(), opener: owner.clone() };
    let Some(scope) = proposed(node, hub, readers, "records")? else { return Ok(None) };
    let error = |error: fastctx_content::Error| error.to_string();
    let Some(holding) = library.holding(&scope).map_err(error)? else { return Ok(None) };
    let device = node.assets.as_ref().map(|assets| assets.device()).unwrap_or_default();
    if library.orderer(&scope, stream).map_err(error)?.is_some_and(|orderer| orderer != device) {
        return Ok(None);
    }
    Ok(Some((scope, holding)))
}

/// Where this look writes: the body this machine is shared whole as on
/// `hub` now, and its owner.
struct Shared<'a> {
    body: Option<String>,
    owner: Option<&'a AccountName>,
}

/// One look, or one mark, at a time: a look keeps the standing it read, so
/// a mark made meanwhile would be written over.
static LOOKING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One look at what was done here for the accounts of `hub`.
fn look(
    node: &Arc<Node>,
    hub: &HubId,
    home: Option<&AccountName>,
    waits: bool,
    owner: Option<&AccountName>,
) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let _looking = LOOKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let machine: String = assets.device()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    let shared = Shared { body: crate::timeline::born_shared::shared_body(node, hub), owner };
    came_into_force(node, hub, shared.body.as_deref())?;
    deeds(node, hub, home, waits, &shared, &format!("{}/{machine}", AccountActivity::STREAM))?;
    forgotten(node, hub, &format!("{}/{machine}", AccountActivity::STREAM))?;
    hours(node, hub, home, &format!("{}/{machine}", AccountActivity::STREAM))?;
    usage(node, hub, home, waits, &shared, &format!("{}/{machine}", AccountUsage::STREAM))
}

/// Lets go of the deeds of conversations deleted here, as the book let go
/// of their rows and kept their keys (`activity_book::sealing`): in every
/// records scope of `hub` this machine wrote one into, each scope from the
/// key where its own look last stood, once it is pulled whole — a scope
/// held again later, its earlier items pulled back, has them let go of too
/// (codex-847f57fd #4).
fn forgotten(node: &Arc<Node>, hub: &HubId, stream: &str) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    if node.book_database.fault().is_some() {
        return Ok(());
    }
    let name = format!("activity/{hub}");
    let mut at = standing(library, &name)?;
    let error = |error: fastctx_content::Error| error.to_string();
    let device = assets.device();
    let accounts = format!("a/{hub}/");
    for scope in library.held_scopes().map_err(error)? {
        let account = scope.key.starts_with(&accounts)
            && scope.key.rsplit('/').next().is_some_and(|last| last.starts_with("records-"));
        let body = scope.key.starts_with("b/")
            && scope.key.ends_with("/records")
            && scope.hub.as_deref() == Some(hub.as_str());
        if !(account || body) || scope.pulled != Some(scope.holding) {
            continue;
        }
        if library.orderer(&scope.key, stream).map_err(error)?.is_some_and(|orderer| orderer != device) {
            continue;
        }
        // Where this scope's look stood, in this holding period.
        let kept = &at["forgotten"][&scope.key];
        let after =
            if kept["holding"].as_i64() == Some(scope.holding) { kept["after"].as_i64().unwrap_or(0) } else { 0 };
        let keys: Vec<(i64, String)> = node
            .book_database
            .read_now(move |db| {
                Ok(db
                    .prepare_cached("SELECT id, deed FROM book_forgotten WHERE id > ?1 ORDER BY id LIMIT 256")?
                    .query_map([after], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .map_err(|error| error.to_string())?;
        let Some((last, _)) = keys.last().cloned() else { continue };
        let mut writes = Vec::new();
        for (_, deed) in &keys {
            // Under its own key, or, in a body's records, under the digest a
            // deed of a session elsewhere is kept under (`elsewhere`).
            for key in [deed.clone(), elsewhere(deed)] {
                let item = fastctx_content::v2::item_of(&key);
                if library.item(&scope.key, stream, &item).map_err(error)?.is_some() {
                    writes.push(Write::<AccountActivity> {
                        order: item.clone(),
                        item,
                        value: None,
                        objects: Vec::new(),
                    });
                }
            }
        }
        if !writes.is_empty() {
            Writer::<AccountActivity>::new(library, &scope.key, stream)
                .map_err(error)?
                .in_holding(scope.holding)
                .write_all(writes, Vec::new(), false, assets.signer())
                .map_err(error)?;
        }
        at["forgotten"][&scope.key] = serde_json::json!({ "holding": scope.holding, "after": last });
    }
    library.keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)
}

/// An hour the book kept from an earlier version without its deeds.
struct LegacyHour {
    at: i64,
    session: String,
    agent: String,
    held: bool,
    account: String,
    runs: i64,
    failed: i64,
    busy: i64,
    files: i64,
}

/// The order an hour of the book is kept under, beside the deeds.
const HOURS: &str = "hour/";

/// Hours by item, each with its order and body.
type Hours = BTreeMap<Vec<u8>, (Vec<u8>, Vec<u8>)>;

/// The hours the book keeps from an earlier version, whose deeds it never
/// kept (`book_hour`, written by its migration alone, §28.58): written into
/// the activity stream of their accounts as they stand, each under
/// `hour/…`, and let go of as the book lets go of them. Looked at again only
/// as they change or a scope is held anew. Its accounts' own, never a
/// body's: they are of before any sharing was kept.
fn hours(node: &Arc<Node>, hub: &HubId, home: Option<&AccountName>, stream: &str) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    if node.book_database.fault().is_some() {
        return Ok(());
    }
    let rows: Vec<LegacyHour> = node
        .book_database
        .read_now(|db| {
            Ok(db
                .prepare_cached(
                    "SELECT at, session, agent, held, account, runs, failed, busy, files FROM book_hour \
                     ORDER BY at, session, agent, held, account",
                )?
                .query_map([], |row| {
                    Ok(LegacyHour {
                        at: row.get(0)?,
                        session: row.get(1)?,
                        agent: row.get(2)?,
                        held: row.get::<_, i64>(3)? != 0,
                        account: row.get(4)?,
                        runs: row.get(5)?,
                        failed: row.get(6)?,
                        busy: row.get(7)?,
                        files: row.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(|error| error.to_string())?;
    let error = |error: fastctx_content::Error| error.to_string();
    // Each account's hours, by item, with their bodies.
    let mut by_account: BTreeMap<AccountName, Hours> = BTreeMap::new();
    for hour in &rows {
        let Some(account) = routed(&hour.account, hub, home) else { continue };
        // The book rounds what was done down to its hour: an hour that ends
        // after the account was opened may hold what was done for it
        // (codex-357de52a, third round #5).
        if !of_this(node, hub, &account, hour.at.saturating_add(HOUR_MS - 1)) {
            continue;
        }
        let key = format!("{HOURS}{}/{}/{}/{}/{}", hour.at, hour.session, hour.agent, hour.held, hour.account);
        // Named by its key as a deed is (`AccountActivity::authorize`).
        let body = serde_json::to_vec(&serde_json::json!({
            "deed": key,
            "hour": true,
            "at": hour.at,
            "session": hour.session,
            "agent": hour.agent,
            "held": hour.held,
            "account": hour.account,
            "runs": hour.runs,
            "failed": hour.failed,
            "busy": hour.busy,
            "files": hour.files,
        }))
        .map_err(|error| error.to_string())?;
        let order = format!("{HOURS}{:020}", hour.at).into_bytes();
        by_account.entry(account).or_default().insert(fastctx_content::v2::item_of(&key), (order, body));
    }
    // The deeds folded into those hours, which a copy of before may hold
    // beside them: let go of, as the book tells them by their hour alone.
    let folded: Vec<Vec<u8>> = node
        .book_database
        .read_now(|db| {
            Ok(db
                .prepare_cached("SELECT deed FROM book_folding WHERE applied = 1 ORDER BY deed")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(|error| error.to_string())?
        .iter()
        .map(|deed| fastctx_content::v2::item_of(deed))
        .collect();
    let name = format!("activity-hours/{hub}");
    let mut at = standing(library, &name)?;
    // Each scope to write: those of the accounts with hours now, and those
    // written before whose hours all went since (codex-357de52a #6).
    let mut targets: BTreeMap<String, (i64, Hours)> = BTreeMap::new();
    for (account, wanted) in by_account {
        if let Some((scope, holding)) = writable(node, library, hub, &account, stream)? {
            targets.insert(scope, (holding, wanted));
        }
    }
    let device = assets.device();
    let before: Vec<String> =
        at["written"].as_object().map(|written| written.keys().cloned().collect()).unwrap_or_default();
    for scope in before {
        if targets.contains_key(&scope) {
            continue;
        }
        let Some(holding) = library.holding(&scope).map_err(error)? else { continue };
        if library.orderer(&scope, stream).map_err(error)?.is_some_and(|orderer| orderer != device) {
            continue;
        }
        targets.insert(scope, (holding, BTreeMap::new()));
    }
    for (scope, (holding, wanted)) in targets {
        use sha2::Digest as _;
        let mut digest = sha2::Sha256::new();
        digest.update(holding.to_be_bytes());
        for (item, (_, body)) in &wanted {
            digest.update(item);
            digest.update(body);
        }
        for item in &folded {
            digest.update(b"folded");
            digest.update(item);
        }
        let digest: String = digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        if at["written"][&scope].as_str() == Some(digest.as_str()) {
            continue;
        }
        // What the stream holds of them now: let go of where the book did.
        let mut writes = Vec::new();
        let mut after = Some((HOURS.as_bytes().to_vec(), Vec::new()));
        loop {
            let page = library.page(&scope, stream, after.take(), 500).map_err(error)?;
            let full = page.len() == 500;
            let mut past = false;
            for entry in &page {
                if !entry.order.starts_with(HOURS.as_bytes()) {
                    past = true;
                    break;
                }
                match wanted.get(&entry.item) {
                    Some((_, body)) if *body == entry.body => {}
                    Some(_) => {}
                    None => writes.push(Write::<AccountActivity> {
                        order: entry.order.clone(),
                        item: entry.item.clone(),
                        value: None,
                        objects: Vec::new(),
                    }),
                }
            }
            match page.last() {
                Some(last) if full && !past => after = Some((last.order.clone(), last.item.clone())),
                _ => break,
            }
        }
        for (item, (order, body)) in wanted {
            if library.item(&scope, stream, &item).map_err(error)?.is_some_and(|entry| entry.body == body) {
                continue;
            }
            writes.push(Write::<AccountActivity> { order, item, value: Some(body), objects: Vec::new() });
        }
        for item in &folded {
            if let Some(entry) = library.item(&scope, stream, item).map_err(error)? {
                writes.push(Write::<AccountActivity> {
                    order: entry.order,
                    item: item.clone(),
                    value: None,
                    objects: Vec::new(),
                });
            }
        }
        while !writes.is_empty() {
            let batch: Vec<_> = writes.drain(..writes.len().min(256)).collect();
            Writer::<AccountActivity>::new(library, &scope, stream)
                .map_err(error)?
                .in_holding(holding)
                .write_all(batch, Vec::new(), false, assets.signer())
                .map_err(error)?;
        }
        at["written"][&scope] = serde_json::json!(digest);
    }
    library.keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)
}

/// Where this machine is first seen shared whole as `body` on `hub`, all
/// done here up to now is marked its accounts' own (§29, 2.5): what was
/// written after it cleared and before its share came into force too. One
/// written after the share and before this look is taken as its accounts'
/// as well, the safe way.
fn came_into_force(node: &Node, hub: &HubId, body: Option<&str>) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    let name = format!("activity/{hub}");
    let seen = standing(library, &name)?["shared_as"].as_str().map(str::to_string);
    if seen.as_deref() == body {
        return Ok(());
    }
    if body.is_some() {
        mark(node, hub)?;
    }
    let mut at = standing(library, &name)?;
    at["shared_as"] = body.map_or(serde_json::Value::Null, |body| serde_json::json!(body));
    library
        .keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

/// Marks where the book and each usage file stand for `hub` now, as this
/// machine clears for a share: nothing up to here goes to a body (§29, 2.5).
/// A mark only moves on; where anything could not be looked at, none is
/// made and the machine does not clear. Call on a thread that may block.
pub(crate) fn mark_private(node: &Node, hub: &HubId) -> Result<(), String> {
    let _looking = LOOKING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    mark(node, hub)
}

fn mark(node: &Node, hub: &HubId) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    let error = |error: fastctx_content::Error| error.to_string();
    if let Some(fault) = node.book_database.fault() {
        return Err(format!("what this machine did cannot be read to mark it its accounts' own: {fault}"));
    }
    {
        let last: Option<(i64, String)> = node
            .book_database
            .read_now(|db| {
                use fastctx_storage::rusqlite::OptionalExtension as _;
                Ok(db
                    .prepare_cached("SELECT began, deed FROM book_deeds ORDER BY began DESC, deed DESC LIMIT 1")?
                    .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .optional()?)
            })
            .map_err(|error| error.to_string())?;
        if let Some((began, deed)) = last {
            let name = format!("activity/{hub}");
            let mut at = standing(library, &name)?;
            let was = (
                at["private"]["began"].as_i64().unwrap_or(i64::MIN),
                at["private"]["deed"].as_str().unwrap_or_default().to_string(),
            );
            if (began, deed.clone()) > was {
                at["private"] = serde_json::json!({ "began": began, "deed": deed });
                library
                    .keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?)
                    .map_err(error)?;
            }
        }
    }
    let name = format!("usage/{hub}");
    let mut at = standing(library, &name)?;
    let mut moved = false;
    for (file, path) in usage_files(node)? {
        let metadata = fastctx_platform::disk::metadata(&path)
            .map_err(|error| format!("looking at {}: {error}", path.display()))?;
        if at["private"][&file].as_u64().is_none_or(|was| was < metadata.len()) {
            at["private"][&file] = serde_json::json!(metadata.len());
            moved = true;
        }
    }
    if moved {
        library.keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)?;
    }
    Ok(())
}

/// Every usage file here, as `<ledger>/<date>` with its path, in order.
fn usage_files(node: &Node) -> Result<Vec<(String, std::path::PathBuf)>, String> {
    let root = node.world.path().join("usage");
    let ledgers = match fastctx_platform::disk::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("looking at {}: {error}", root.display())),
    };
    // Every one, or an error: a file passed by would be marked nobody's.
    let mut files = Vec::new();
    for ledger in ledgers {
        let ledger = ledger.map_err(|error| format!("looking at {}: {error}", root.display()))?;
        let Some(dir) = ledger.file_name().to_str().map(str::to_string) else { continue };
        let days = fastctx_platform::disk::read_dir(ledger.path())
            .map_err(|error| format!("looking at {}: {error}", ledger.path().display()))?;
        for day in days {
            let day = day.map_err(|error| format!("looking at {}: {error}", ledger.path().display()))?;
            let Some(date) = day.file_name().to_str().and_then(|name| name.strip_suffix(".jsonl")).map(str::to_string)
            else {
                continue;
            };
            files.push((format!("{dir}/{date}"), day.path()));
        }
    }
    files.sort();
    Ok(files)
}

/// Where a look stands, kept beside: by network.
fn standing(library: &Library, name: &str) -> Result<serde_json::Value, String> {
    Ok(library
        .checkpoint(name)
        .map_err(|error| error.to_string())?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| serde_json::json!({})))
}

/// Whether any scope written before was held anew since, whether or not
/// anything new is there to write: then everything is looked at again from
/// the start (M18).
fn any_held_anew(library: &Library, standing: &mut serde_json::Value) -> Result<bool, String> {
    let scopes: Vec<(String, i64)> = standing["holdings"]
        .as_object()
        .map(|held| held.iter().filter_map(|(scope, holding)| Some((scope.clone(), holding.as_i64()?))).collect())
        .unwrap_or_default();
    let mut anew = false;
    for (scope, was) in scopes {
        match library.holding(&scope).map_err(|error| error.to_string())? {
            Some(holding) if holding == was => {}
            Some(holding) => {
                standing["holdings"][&scope] = serde_json::json!(holding);
                anew = true;
            }
            None => {
                if let Some(held) = standing["holdings"].as_object_mut() {
                    held.remove(&scope);
                }
                anew = true;
            }
        }
    }
    Ok(anew)
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
    /// What the machine was shared as when the book first kept it (7965
    /// P08); none for a deed kept before that was kept.
    sharing: Option<Marked>,
}

/// A deed not folded into an hour by an earlier version.
const UNFOLDED: &str = "NOT EXISTS (SELECT 1 FROM book_folding f WHERE f.deed = d.deed AND f.applied = 1)";

const DEED_COLUMNS: &str = "d.deed, d.job, d.began, d.ended, d.from_machine, d.session, d.agent, d.held, d.project, d.account, d.script, d.outcome, d.code, d.output, d.went, d.looked, d.copied, EXISTS(SELECT 1 FROM book_complete c WHERE c.deed = d.deed), d.sharing";

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
    let sharing = row.get::<_, Option<String>>(18)?.and_then(|text| serde_json::from_str(&text).ok());
    Ok(Deed { key, began, account, complete: row.get(17)?, body, sharing })
}

/// Writes the deeds since the last look, and those still read, of the
/// accounts of `hub`.
fn deeds(
    node: &Arc<Node>,
    hub: &HubId,
    home: Option<&AccountName>,
    waits: bool,
    shared: &Shared<'_>,
    stream: &str,
) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    if node.book_database.fault().is_some() {
        return Ok(());
    }
    let name = format!("activity/{hub}");
    let mut at = standing(library, &name)?;
    if any_held_anew(library, &mut at)? {
        at["began"] = serde_json::json!(i64::MIN);
        at["deed"] = serde_json::json!("");
    }
    let (began, after) =
        (at["began"].as_i64().unwrap_or(i64::MIN), at["deed"].as_str().unwrap_or_default().to_string());
    let open: BTreeSet<String> =
        at["open"].as_array().into_iter().flatten().filter_map(|deed| deed.as_str().map(str::to_string)).collect();
    // Up to where the book stood as this machine cleared for a share: its
    // accounts' own, never the body's.
    let private = (
        at["private"]["began"].as_i64().unwrap_or(i64::MIN),
        at["private"]["deed"].as_str().unwrap_or_default().to_string(),
    );
    let (found, touched) = node
        .book_database
        .read_now(move |db| {
            let mut found = Vec::new();
            // A deed an earlier version folded into its hour is told by the
            // hour alone (`hours`), as the book reads it (`query::hours`).
            let mut new = db.prepare_cached(&format!(
                "SELECT {DEED_COLUMNS} FROM book_deeds d WHERE (d.began, d.deed) > (?1, ?2) AND {UNFOLDED} ORDER BY d.began, d.deed LIMIT ?3"
            ))?;
            for deed in new.query_map(params![began, after, DEEDS], deed_of)? {
                found.push(deed?);
            }
            let mut again = db.prepare_cached(&format!("SELECT {DEED_COLUMNS} FROM book_deeds d WHERE d.deed = ?1 AND {UNFOLDED}"))?;
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
    type Writes = (i64, Vec<Write<AccountActivity>>, Vec<fastctx_content::v2::Lease>);
    let mut writes: BTreeMap<String, Writes> = BTreeMap::new();
    let (mut still_open, mut anew) = (BTreeSet::new(), false);
    let (mut last_began, mut last_deed) = (began, at["deed"].as_str().unwrap_or_default().to_string());
    for mut deed in found {
        if deed.began > last_began || (deed.began == last_began && deed.key > last_deed) {
            (last_began, last_deed) = (deed.began, deed.key.clone());
        }
        let before_shared = (deed.began, deed.key.clone()) <= private;
        // A body's as the machine was shared whole when the book kept it,
        // however it is shared now; one kept before that was kept goes as
        // the sharing stands now, its accounts' own up to the mark.
        let body = match &deed.sharing {
            Some(sharing) => sharing.get(hub.as_str()).cloned().flatten(),
            None => shared.body.clone().filter(|_| !before_shared),
        };
        let to_body = body.is_some();
        let target = match body.as_deref() {
            Some(body) => body_scope(node, library, hub, body, shared.owner, stream)?,
            None => {
                let Some(account) = routed(&deed.account, hub, home) else {
                    // The machine's own user's, its owner not logged in now:
                    // looked at again, not passed by (OO17).
                    if deed.account.is_empty() && waits {
                        still_open.insert(deed.key.clone());
                    }
                    continue;
                };
                if !of_this(node, hub, &account, deed.began) {
                    continue;
                }
                writable(node, library, hub, &account, stream)?
            }
        };
        let Some((scope, holding)) = target else {
            // Its scope not held yet: looked at again.
            still_open.insert(deed.key.clone());
            continue;
        };
        // Whose it is, as a body's records hold several accounts'.
        deed.body["account"] = serde_json::json!(deed.account);
        anew |= held_anew(&mut at, &scope, holding);
        if !deed.complete {
            still_open.insert(deed.key.clone());
        }
        deed.body["touched"] = serde_json::json!(touched.get(&deed.key).cloned().unwrap_or_default());
        // What a session elsewhere ran here names, in a body's records,
        // neither that session nor its deed by it: whose conversation it was
        // is its account's, not the body's (§28.47).
        let mut key = deed.key.clone();
        if to_body && deed.body.get("from").is_some_and(|from| !from.is_null()) {
            key = elsewhere(&deed.key);
            deed.body["session"] = serde_json::Value::Null;
            deed.body["deed"] = serde_json::json!(key);
            // Kept under its own key by an earlier version: that one goes, or
            // the deed is counted twice.
            let raw = fastctx_content::v2::item_of(&deed.key);
            if library.item(&scope, stream, &raw).map_err(error)?.is_some() {
                let scoped = writes.entry(scope.clone()).or_insert_with(|| (holding, Vec::new(), Vec::new()));
                scoped.1.push(Write { order: raw.clone(), item: raw, value: None, objects: Vec::new() });
            }
        }
        let body = serde_json::to_vec(&deed.body).map_err(|error| error.to_string())?;
        // A key longer than an item's: named by its digest, the whole key in
        // the body (PP13, N09).
        let item = fastctx_content::v2::item_of(&key);
        use sha2::Digest as _;
        let digest: [u8; 32] = sha2::Sha256::digest(&body).into();
        if library.item(&scope, stream, &item).map_err(error)?.is_some_and(|entry| match entry.objects.first() {
            None => entry.body == body,
            Some(object) => object.hash == digest,
        }) {
            continue;
        }
        // A deed larger than a body holds — a job that touched very many
        // files — is its one object (M17).
        let scoped = writes.entry(scope).or_insert_with(|| (holding, Vec::new(), Vec::new()));
        let (value, objects) = if body.len() > BODY_MOST {
            let lease = library.put(&mut body.as_slice(), "activity", LEASE).map_err(error)?;
            let said = serde_json::json!({ "deed": key, "in_object": true, "bytes": body.len() });
            scoped.2.push(lease);
            (serde_json::to_vec(&said).map_err(|error| error.to_string())?, vec![lease.object])
        } else {
            (body, Vec::new())
        };
        scoped.1.push(Write { order: item.clone(), item, value: Some(value), objects });
    }
    for (scope, (holding, writes, leases)) in writes {
        Writer::<AccountActivity>::new(library, &scope, stream)
            .map_err(error)?
            .in_holding(holding)
            .write_all(writes, leases, false, assets.signer())
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
fn usage(
    node: &Arc<Node>,
    hub: &HubId,
    home: Option<&AccountName>,
    waits: bool,
    shared: &Shared<'_>,
    stream: &str,
) -> Result<(), String> {
    let Some(assets) = node.assets.as_ref() else { return Ok(()) };
    let library = &assets.library;
    let files = usage_files(node)?;
    let name = format!("usage/{hub}");
    let mut at = standing(library, &name)?;
    if any_held_anew(library, &mut at)? {
        at["files"] = serde_json::json!({});
        at["marks"] = serde_json::json!({});
    }
    let error = |error: fastctx_content::Error| error.to_string();
    let (mut budget, mut anew) = (USAGE_BYTES, false);
    'files: for (file, path) in files {
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
        let mut bytes = &bytes[..=end];
        // As the ledger marked it shared as each line was written (7965
        // P08): the piece goes on to where that changes for this network.
        let started = marks_at(&at, &file, &path, offset)?;
        let (run, marked) = marked_run(started.clone(), bytes, hub.as_str());
        bytes = &bytes[..run];
        // Up to where the file stood as this machine cleared for a share:
        // its accounts' own, never the body's — where nothing is marked.
        let mark = at["private"][&file].as_u64().filter(|mark| *mark > offset && marked.is_none());
        if let Some(mark) = mark
            && ((mark - offset) as usize) < bytes.len()
        {
            let kept = &bytes[..(mark - offset) as usize];
            let Some(end) = kept.iter().rposition(|&byte| byte == b'\n') else { continue };
            bytes = &kept[..=end];
        }
        // A body's as marked; where nothing is, shared whole now: the piece,
        // any account's lines, goes to the body.
        let to_body = match &marked {
            Some(marked) => marked.clone(),
            None => shared.body.clone().filter(|_| mark.is_none()),
        };
        if let Some(body) = to_body.as_deref() {
            let Some((scope, holding)) = body_scope(node, library, hub, body, shared.owner, stream)? else { continue };
            if held_anew(&mut at, &scope, holding) {
                anew = true;
                break 'files;
            }
            let item = format!("{file}/{offset:016x}").into_bytes();
            let written = library.item(&scope, stream, &item).map_err(error)?;
            // Written before a stop that kept no standing: it ends where it
            // ended then, the rest a piece of its own (M15).
            if let Some(source) = written
                .as_ref()
                .and_then(|entry| serde_json::from_slice::<serde_json::Value>(&entry.body).ok())
                .and_then(|body| body.get("source").and_then(serde_json::Value::as_u64))
                .filter(|source| *source > 0 && (*source as usize) < bytes.len())
            {
                bytes = &bytes[..source as usize];
            }
            if written.is_none() {
                let lines = for_body(node, bytes);
                let lease = library.put(&mut &lines[..], "usage", LEASE).map_err(error)?;
                let body = serde_json::to_vec(
                    &serde_json::json!({ "file": file, "offset": offset, "length": lines.len(), "source": bytes.len() }),
                )
                .map_err(|error| error.to_string())?;
                Writer::<AccountUsage>::new(library, &scope, stream)
                    .map_err(error)?
                    .in_holding(holding)
                    .write_all(
                        vec![Write { order: item.clone(), item, value: Some(body), objects: vec![lease.object] }],
                        vec![lease],
                        false,
                        assets.signer(),
                    )
                    .map_err(error)?;
            }
            at["files"][&file] = serde_json::json!(offset + bytes.len() as u64);
            at["marks"][&file] = serde_json::json!(marks_through(started, bytes));
            budget = budget.saturating_sub(bytes.len() as u64);
            continue;
        }
        // Each account's lines; and whether any is the machine's own user's
        // with its owner not logged in now, which waits (OO17). A line used
        // for several accounts at once books each an even share (`usage
        // ledger::uses`): each of the others gets it too, naming no
        // conversation of the first's (§28.58).
        let split = |bytes: &[u8]| {
            let mut lines: BTreeMap<AccountName, Vec<u8>> = BTreeMap::new();
            let mut unowned = false;
            for line in bytes.split_inclusive(|&byte| byte == b'\n') {
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else { continue };
                // A mark of how the machine is shared is no account's line.
                if value.get("kind").and_then(serde_json::Value::as_str) == Some("shared") {
                    continue;
                }
                let named = |who: &serde_json::Value| -> Option<Option<AccountName>> {
                    let (of, name) = (who["hub"].as_str()?, who["account"].as_str()?);
                    Some(routed(&format!("{of}/{name}"), hub, home))
                };
                // Done before an account of its name was opened: that one's
                // made again since, and none of this line goes to it.
                let at = value
                    .get("at")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|at| at.parse::<jiff::Timestamp>().ok());
                let fits = |account: &AccountName| at.is_none_or(|at| of_this(node, hub, account, at.as_millisecond()));
                let account = match value.get("who").filter(|who| !who.is_null()) {
                    Some(who) => {
                        let Some(account) = named(who) else { continue };
                        account
                    }
                    None => {
                        unowned |= waits;
                        home.cloned()
                    }
                };
                let sharing: BTreeSet<AccountName> = value
                    .get("with")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|who| named(who).flatten())
                    .filter(|other| Some(other) != account.as_ref())
                    .filter(|other| fits(other))
                    .collect();
                if let Some(account) = account.filter(|account| fits(account)) {
                    lines.entry(account).or_default().extend_from_slice(line);
                }
                if !sharing.is_empty() {
                    let mut shared = value.clone();
                    unname_all(&mut shared);
                    let Ok(mut text) = serde_json::to_vec(&shared) else { continue };
                    text.push(b'\n');
                    for other in sharing {
                        lines.entry(other).or_default().extend_from_slice(&text);
                    }
                }
            }
            (lines, unowned)
        };
        let (mut lines, mut unowned) = split(bytes);
        // A piece written here before a stop that kept no standing ends
        // where it ended then, however the file grew since: read again to
        // there, the rest comes as a piece of its own (M15).
        let item = format!("{file}/{offset:016x}").into_bytes();
        let mut ended = None;
        for account in lines.keys() {
            let Some((scope, _)) = writable(node, library, hub, account, stream)? else { continue };
            if let Some(entry) = library.item(&scope, stream, &item).map_err(error)?
                && let Some(source) = serde_json::from_slice::<serde_json::Value>(&entry.body)
                    .ok()
                    .and_then(|body| body.get("source").and_then(serde_json::Value::as_u64))
            {
                ended = Some(source as usize);
                break;
            }
        }
        if let Some(ended) = ended.filter(|ended| *ended > 0 && *ended < bytes.len()) {
            bytes = &bytes[..ended];
            (lines, unowned) = split(bytes);
        }
        let mut waiting = unowned;
        for (account, lines) in lines {
            let Some((scope, holding)) = writable(node, library, hub, &account, stream)? else {
                waiting = true;
                continue;
            };
            // Held anew since the look began: nothing is written on from
            // where the earlier holding stood; the next look begins at each
            // file's start (PP12).
            if held_anew(&mut at, &scope, holding) {
                anew = true;
                break 'files;
            }
            if library.item(&scope, stream, &item).map_err(error)?.is_some() {
                continue;
            }
            let lease = library.put(&mut lines.as_slice(), "usage", LEASE).map_err(error)?;
            let body = serde_json::to_vec(
                &serde_json::json!({ "file": file, "offset": offset, "length": lines.len(), "source": bytes.len() }),
            )
            .map_err(|error| error.to_string())?;
            Writer::<AccountUsage>::new(library, &scope, stream)
                .map_err(error)?
                .in_holding(holding)
                .write_all(
                    vec![Write {
                        order: item.clone(),
                        item: item.clone(),
                        value: Some(body),
                        objects: vec![lease.object],
                    }],
                    vec![lease],
                    false,
                    assets.signer(),
                )
                .map_err(error)?;
        }
        // A scope not held yet: this piece is read again next time.
        if waiting {
            continue;
        }
        at["files"][&file] = serde_json::json!(offset + bytes.len() as u64);
        at["marks"][&file] = serde_json::json!(marks_through(started, bytes));
        budget = budget.saturating_sub(bytes.len() as u64);
    }
    if anew {
        at["files"] = serde_json::json!({});
        at["marks"] = serde_json::json!({});
    }
    library.keep_checkpoint(&name, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)
}

/// The lines of a piece of the usage ledger as a body's records keep them:
/// a call made for a conversation an account was assisted in names neither
/// that conversation nor its turn, the conversation being that account's
/// alone (`placement-one-rule` 2026-11-10); the usage answer hides them
/// alike from any other (`usage::calls`). Who used how much is said still.
fn for_body(node: &Node, bytes: &[u8]) -> Vec<u8> {
    let mut lines = Vec::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        let unnamed = serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|mut value| unname(node, &mut value).then_some(value))
            .and_then(|value| serde_json::to_vec(&value).ok());
        match unnamed {
            Some(text) => {
                lines.extend_from_slice(&text);
                lines.push(b'\n');
            }
            None => lines.extend_from_slice(line),
        }
    }
    lines
}

/// The key a deed of a session elsewhere is kept under in a body's
/// records: its digest, naming neither that session nor the deed by it
/// (§28.47).
fn elsewhere(key: &str) -> String {
    use sha2::Digest as _;
    let digest: String =
        sha2::Sha256::digest(key.as_bytes()).iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("elsewhere {digest}")
}

/// Takes out of `value`, at any depth, every conversation and turn: a line
/// another account shares names nothing of the conversation it was used in
/// (§28.58). Whom it worked for stays, its accounts named already.
fn unname_all(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for name in ["conversation", "turn"] {
                map.remove(name);
            }
            map.values_mut().for_each(unname_all);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(unname_all),
        _ => {}
    }
}

/// Takes out of `value`, at any depth, the conversation and turn of each
/// call made for a conversation an account was assisted in here; whether
/// it took any.
fn unname(node: &Node, value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let assisted = map
                .get("conversation")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| id.parse::<fastctx_types::ResidentId>().ok())
                .is_some_and(|id| match crate::assists::host::kept(node, &id) {
                    Ok(Some(assisted)) => assisted,
                    // Its record gone with the conversation: whether it was
                    // assisted is not known — who holds this machine now
                    // says nothing of then — so it is not named (§28.47).
                    _ => true,
                });
            if assisted {
                map.remove("conversation");
                map.remove("turn");
            }
            map.values_mut().fold(assisted, |changed, inner| unname(node, inner) || changed)
        }
        serde_json::Value::Array(items) => items.iter_mut().fold(false, |changed, item| unname(node, item) || changed),
        _ => false,
    }
}
