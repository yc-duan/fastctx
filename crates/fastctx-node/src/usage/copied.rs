//! Another machine's usage, from the copies here while it is away (plan
//! 3-5, design §28.27). Its usage lines go into each account's records
//! scope by the account they are for, the lines of no account into its
//! owner's, and all of them, while it is shared whole, into its body's
//! records scope (`assets::activity` `usage`): each piece the lines of one
//! day's file `<agent>-<login key>/<day>` from an offset. While the machine
//! is away from every network both are on, a login's report is made from
//! the pieces the accounts logged in here read, put back together into the
//! day files of a ledger in a passing folder and read as the machine reads
//! its own. What the copies do not hold — other accounts' lines — is not in
//! it.

use crate::Node;
use fastctx_content::v2::{AccountUsage, Kind as _};
use fastctx_types::access::{Acting, HubAccount, Written};
use fastctx_types::usage::UsageReport;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// A ledger's day files as pieces: by day, each piece by its offset and the
/// scope it came from, its lines.
type Days = BTreeMap<String, BTreeMap<(u64, String), Vec<u8>>>;

/// What a report from the copies was asked for: the node's root, the
/// machine, its network, the login, and whose use is shown.
type Asked = (PathBuf, String, String, String, PathBuf, Option<HubAccount>, bool);

/// Each copy the report was read from, by its scope, at the revision read.
type Read = Vec<(String, u64)>;

/// A report made, from what it was read from, and when.
type Made = (Read, Instant, UsageReport);

/// Reports made from the copies, kept while the copies stand as they were
/// read: a page asks each minute, and making one reads every piece of the
/// login's ledger again. Kept a while at most all the same, as what one
/// says of now (today, the week) moves.
static MADE: LazyLock<Mutex<HashMap<Asked, Made>>> = LazyLock::new(Mutex::default);
const MADE_FOR: Duration = Duration::from_secs(300);
/// The most reports kept.
const MADE_KEPT: usize = 16;

/// `agent`'s login `dir` on `written` (of the network it names, or the one
/// network that lists it) as the copies here hold it, while the machine is
/// away, shown as the machine would show the accounts logged in here:
/// everyone's use where one of them may work on it, else the one account's
/// own; none when it is not away, its name could be two machines, the
/// accounts here are several and none may work on it, the login's path
/// could name two of its logins, a piece is not all here, or no copy here
/// holds any of it.
pub(crate) fn report(node: &Node, written: &Written, agent: &str, dir: &Path) -> Option<UsageReport> {
    let machine = &written.name;
    if !crate::timeline::copy::away(node, machine) {
        return None;
    }
    let library = &node.assets.as_ref()?.library;
    // The one network it is on as asked: its stream there by its key, who
    // is logged in here, whether one of them owns it, and may work on it.
    let mut found = None;
    for link in node.links() {
        if written.hub.as_ref().is_some_and(|hub| link.hub_name() != *hub) {
            continue;
        }
        let view = link.view();
        let Some(member) = view.member(machine) else { continue };
        let named: String = member.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        let accounts = node.accounts_on(&link, &Acting::Here);
        let owner = member.home.as_ref().is_some_and(|home| accounts.contains(home));
        let hands = accounts.iter().any(|account| view.rights(account, machine).hands);
        let hub = link.membership().id.clone();
        if found.replace((hub, format!("{}/{named}", AccountUsage::STREAM), accounts, owner, hands)).is_some() {
            return None;
        }
    }
    let (hub, stream, accounts, owner, hands) = found?;
    // Everyone's, or the one account's own, as the machine answers one that
    // may not work on it (`logins::remote`, `only`).
    let only = match (hands, accounts.as_slice()) {
        (true, _) => None,
        (false, [account]) => Some(HubAccount { hub: hub.clone(), account: account.clone() }),
        (false, _) => return None,
    };
    let scopes: Vec<_> = library
        .held_scopes()
        .ok()?
        .into_iter()
        .filter(|scope| crate::timeline::copy::records_here(node, scope) && scope.hub.as_deref() == Some(hub.as_str()))
        .collect();
    // Made a moment ago of the copies as they stand: as made then.
    let read: Option<Read> = scopes
        .iter()
        .map(|scope| Some((scope.key.clone(), library.view_revision(&scope.key, &stream).ok()?.unwrap_or(0))))
        .collect();
    let asked: Asked = (node.root.clone(), machine.to_string(), hub.to_string(), agent.to_string(), dir.to_path_buf(), only.clone(), owner);
    if let Some(read) = &read
        && let Some((kept, at, report)) = MADE.lock().expect("reports made").get(&asked)
        && kept == read
        && at.elapsed() < MADE_FOR
    {
        return Some(report.clone());
    }
    // Each day's pieces by offset, the lines of each scope's piece in turn,
    // of each key the login's path may go by.
    let mut by_key: BTreeMap<String, Days> = BTreeMap::new();
    let ledgers: BTreeMap<String, String> = keys(dir).into_iter().map(|key| (format!("{agent}-{key}/"), key)).collect();
    for scope in &scopes {
        let mut after = None;
        loop {
            let page = library.page(&scope.key, &stream, after.take(), 500).ok()?;
            let full = page.len() == 500;
            for entry in &page {
                let Ok(body) = serde_json::from_slice::<serde_json::Value>(&entry.body) else { continue };
                let Some(file) = body.get("file").and_then(serde_json::Value::as_str) else { continue };
                let Some((day, key)) = ledgers.iter().find_map(|(ledger, key)| file.strip_prefix(ledger.as_str()).map(|day| (day, key)))
                else {
                    continue;
                };
                // A day's file is named by its date alone.
                if day.len() != 10 || !day.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-') {
                    continue;
                }
                let offset = body.get("offset").and_then(serde_json::Value::as_u64).unwrap_or(0);
                let [object] = entry.objects.as_slice() else { return None };
                // A piece not all here: no whole answer.
                let lines = crate::timeline::copy::object_bytes(library, object)?;
                by_key.entry(key.clone()).or_default().entry(day.to_string()).or_default().insert((offset, scope.key.clone()), lines);
            }
            match page.last() {
                Some(last) if full => after = Some((last.order.clone(), last.item.clone())),
                _ => break,
            }
        }
    }
    // The path read two ways names two of its logins: which is meant is the
    // machine's to say.
    if by_key.len() > 1 {
        return None;
    }
    let days = by_key.into_values().next()?;
    if days.is_empty() {
        return None;
    }
    let folder = node.world.path().join("passing").join(format!(
        "usage-{}",
        u64::from(std::process::id()) ^ jiff::Timestamp::now().as_nanosecond() as u64
    ));
    let made = (|| {
        fastctx_platform::disk::create_dir_all(&folder)?;
        for (day, pieces) in &days {
            let lines: Vec<u8> = pieces.values().flatten().copied().collect();
            fastctx_platform::disk::write(folder.join(format!("{day}.jsonl")), &lines)?;
        }
        super::report::report_of(agent, &folder, only.as_ref(), owner)
    })();
    let _ = fastctx_platform::disk::remove_dir_all(&folder);
    let made = made.ok()?;
    if let Some(read) = read {
        let mut kept = MADE.lock().expect("reports made");
        if kept.len() >= MADE_KEPT && !kept.contains_key(&asked) {
            let oldest = kept.iter().min_by_key(|(_, (_, at, _))| *at).map(|(asked, _)| asked.clone());
            kept.remove(&oldest.expect("one kept"));
        }
        kept.insert(asked, (read, Instant::now(), made.clone()));
    }
    Some(made)
}

/// The keys the login `dir` may go by on the machine it is on, which keys
/// it as its own system writes paths (`homes::key`): as Windows does,
/// separators turned and case folded, and as the others do, as written.
fn keys(dir: &Path) -> BTreeSet<String> {
    use sha2::Digest as _;
    let written = dir.to_string_lossy();
    let windows = written.replace('\\', "/").to_lowercase();
    let windows = windows
        .strip_prefix("//?/unc/")
        .map(|rest| format!("//{rest}"))
        .or_else(|| windows.strip_prefix("//?/").map(str::to_string))
        .unwrap_or(windows);
    [written.trim_end_matches('/').to_string(), windows.trim_end_matches('/').to_string()]
        .into_iter()
        .map(|text| sha2::Sha256::digest(text.as_bytes())[..4].iter().map(|byte| format!("{byte:02x}")).collect())
        .collect()
}
