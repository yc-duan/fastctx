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
use fastctx_types::MachineName;
use fastctx_types::access::Acting;
use fastctx_types::usage::UsageReport;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// `agent`'s login `dir` on `machine` as the copies here hold it, while the
/// machine is away; none when it is not, or no copy here holds any of it.
pub(crate) fn report(node: &Node, machine: &MachineName, agent: &str, dir: &Path) -> Option<UsageReport> {
    if !crate::timeline::copy::away(node, machine) {
        return None;
    }
    let library = &node.assets.as_ref()?.library;
    // Its stream by the key it has on each network that lists it, and
    // whether an account logged in here owns it.
    let (mut streams, mut owner) = (BTreeSet::new(), false);
    for link in node.links() {
        if let Some(member) = link.view().member(machine) {
            let named: String = member.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
            streams.insert(format!("{}/{named}", AccountUsage::STREAM));
            let accounts = node.accounts_on(&link, &Acting::Here);
            owner |= member.home.as_ref().is_some_and(|home| accounts.contains(home));
        }
    }
    let ledgers: BTreeSet<String> = keys(dir).into_iter().map(|key| format!("{agent}-{key}/")).collect();
    // Each day's pieces by offset, the lines of each scope's piece in turn.
    let mut days: BTreeMap<String, BTreeMap<(u64, String), Vec<u8>>> = BTreeMap::new();
    for scope in library.held_scopes().ok()? {
        if !crate::timeline::copy::records_here(node, &scope) {
            continue;
        }
        for stream in &streams {
            let mut after = None;
            loop {
                let page = library.page(&scope.key, stream, after.take(), 500).ok()?;
                let full = page.len() == 500;
                for entry in &page {
                    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&entry.body) else { continue };
                    let Some(day) = body
                        .get("file")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|file| ledgers.iter().find_map(|ledger| file.strip_prefix(ledger.as_str())))
                    else {
                        continue;
                    };
                    // A day's file is named by its date alone.
                    if day.len() != 10 || !day.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-') {
                        continue;
                    }
                    let offset = body.get("offset").and_then(serde_json::Value::as_u64).unwrap_or(0);
                    let [object] = entry.objects.as_slice() else { continue };
                    let Some(lines) = crate::timeline::copy::object_bytes(library, object) else { continue };
                    days.entry(day.to_string()).or_default().insert((offset, scope.key.clone()), lines);
                }
                match page.last() {
                    Some(last) if full => after = Some((last.order.clone(), last.item.clone())),
                    _ => break,
                }
            }
        }
    }
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
        super::report::report_of(agent, &folder, owner)
    })();
    let _ = fastctx_platform::disk::remove_dir_all(&folder);
    made.ok()
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
