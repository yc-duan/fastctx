//! Each sentinel's wakes into its conversation's scope, by the machine it
//! lives on (design §29 2.7, registry 9 `conversation.sentinel-run`): what
//! a wake left — the state it committed (`history/<sequence>.json`) and,
//! where kept, its run (`runs/<sequence>.json`) — as a record of the stream
//! `sentinel-runs/<this machine's key's first 8 bytes>`, named by the
//! sentinel, its generation and the wake's sequence. Only committed wakes
//! are written, and written once. Looked at every minute, each sentinel
//! from the wake after its last one written, a bounded number in a look; a
//! conversation whose scope this machine does not hold is not written.

use crate::Node;
use fastctx_content::v2::{ConversationSentinelRun, Kind as _, LEASE, Library, Write, Writer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use sha2::Digest as _;
use std::time::Duration;

/// How often the sentinels here are looked at.
const EVERY: Duration = Duration::from_secs(60);
/// The most wakes written in one look.
const A_LOOK: usize = 64;

/// Starts writing the wakes of the sentinels here every minute.
pub(crate) fn start(node: &Arc<Node>) {
    let weak: Weak<Node> = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let Some(node) = weak.upgrade() else { return };
            if node.assets.is_none() {
                continue;
            }
            let working = node.clone();
            match fastctx_platform::task::blocking(move || look(&working)).await {
                Ok(()) => {}
                Err(error) => node.log.line(format_args!("writing the sentinels' wakes into the asset library: {error}")),
            }
        }
    });
}

/// The directories under `path`, in order; none when it is not there.
fn directories(path: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fastctx_platform::disk::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("looking at {}: {error}", path.display())),
    };
    let mut found: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| fastctx_platform::disk::is_dir(path)).collect();
    found.sort();
    Ok(found)
}

/// Writes what is new of every sentinel here: `<world>/sentinels/<account's
/// key>/<sentinel>/<generation>/history/<sequence>.json`.
fn look(node: &Arc<Node>) -> Result<(), String> {
    let mut left = A_LOOK;
    for account in directories(&node.world.path().join("sentinels"))? {
        for sentinel in directories(&account)? {
            for generation in directories(&sentinel)? {
                if left == 0 {
                    return Ok(());
                }
                left -= generation_wakes(node, &generation, left)?;
            }
        }
    }
    Ok(())
}

/// Writes at most `most` of the wakes of the generation kept at `directory`
/// not written yet: how many it wrote.
fn generation_wakes(node: &Arc<Node>, directory: &Path, most: usize) -> Result<usize, String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(0) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let history = directory.join("history");
    let entries = match fastctx_platform::disk::read_dir(&history) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("looking at {}: {error}", history.display())),
    };
    let mut wakes: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let sequence = name.to_str()?.strip_suffix(".json")?.parse().ok()?;
            Some((sequence, entry.path()))
        })
        .collect();
    wakes.sort();
    let Some((_, last)) = wakes.last() else { return Ok(0) };
    // The sentinel as its last wake left it: whose conversation, on which
    // network.
    let state: serde_json::Value =
        serde_json::from_slice(&fastctx_platform::disk::read(last).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    let definition = &state["definition"];
    let (Some(id), Some(generation), Some(account), Some(resident), Some(machine)) = (
        definition["id"].as_str(),
        definition["generation"].as_u64(),
        definition["account"].as_str().and_then(|account| fastctx_types::AccountName::try_from(account.to_string()).ok()),
        definition["owner"]["resident"].as_str(),
        definition["owner"]["machine"].as_str().and_then(|machine| fastctx_types::MachineName::try_from(machine.to_string()).ok()),
    ) else {
        return Ok(0);
    };
    // The network it was kept for: the one whose id, with its account, its
    // keeping is named by (`fastctx_sentinel` `scope_key`) — never another
    // of the same name met since (codex-847f57fd #2).
    let kept_as = directory.parent().and_then(Path::parent).and_then(Path::file_name).and_then(|name| name.to_str()).map(str::to_string);
    let Some(link) = node.links().into_iter().find(|link| {
        let scope = fastctx_types::access::HubAccount { hub: link.membership().id.clone(), account: account.clone() };
        let named = serde_json::to_vec(&scope).map(|bytes| sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>());
        named.ok().is_some_and(|named| Some(named) == kept_as)
    }) else {
        return Ok(0);
    };
    let (hub_id, key, now_named) = {
        let view = link.view();
        let Some(me) = view.me() else { return Ok(0) };
        let key: String = me.key.as_bytes()[..8].iter().map(|byte| format!("{byte:02x}")).collect();
        // The machine it lives on by the name it has now, renamed since
        // (codex-847f57fd #12).
        (link.membership().id.to_string(), key, view.directory.canonical(&machine).clone())
    };
    let stream = format!("{}/{key}", ConversationSentinelRun::STREAM);
    // The conversation's scope held here: the one of its network that lives
    // on its machine.
    let mut scopes = library.scopes_ending(&format!("/{resident}")).map_err(error)?.into_iter().filter(|scope| {
        scope.hub.as_deref() == Some(hub_id.as_str())
            && (crate::timeline::copy::lives_on(library, scope, &machine) || crate::timeline::copy::lives_on(library, scope, &now_named))
    });
    let (Some(scope), None) = (scopes.next(), scopes.next()) else { return Ok(0) };
    let Some(holding) = writable(library, &scope.key, &stream, &kept.device())? else { return Ok(0) };
    // From the wake after the last one written in this holding period.
    let standing = format!("sentinel-runs/{}/{id}/{generation}", scope.key);
    let from = library
        .checkpoint(&standing)
        .map_err(error)?
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(|value| value["holding"].as_i64() == Some(holding))
        .and_then(|value| value["next"].as_u64())
        .unwrap_or(0);
    let (mut writes, mut leases, mut next) = (Vec::new(), Vec::new(), from);
    for (sequence, path) in wakes.iter().filter(|(sequence, _)| *sequence >= from) {
        if writes.len() >= most {
            break;
        }
        next = sequence + 1;
        let item = fastctx_content::v2::item_of(&format!("{id}/{generation}/{sequence:020}"));
        if library.item(&scope.key, &stream, &item).map_err(error)?.is_some() {
            continue;
        }
        let bytes = fastctx_platform::disk::read(path).map_err(|error| error.to_string())?;
        let left: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let lease = library.put(&mut bytes.as_slice(), "sentinel-run", LEASE).map_err(error)?;
        let mut objects = vec![lease.object];
        leases.push(lease);
        if let Ok(run) = fastctx_platform::disk::read(directory.join("runs").join(format!("{sequence:020}.json"))) {
            let lease = library.put(&mut run.as_slice(), "sentinel-run", LEASE).map_err(error)?;
            objects.push(lease.object);
            leases.push(lease);
        }
        // How it ended, in a word: the conclusion's own name.
        let conclusion = left["conclusion"]
            .as_object()
            .and_then(|conclusion| conclusion.get("kind").and_then(serde_json::Value::as_str).or_else(|| conclusion.keys().next().map(String::as_str)))
            .or_else(|| left["conclusion"].as_str())
            .map(str::to_string);
        let body = serde_json::to_vec(&serde_json::json!({
            "sentinel": id,
            "generation": generation,
            "sequence": sequence,
            "wake": left["wake_id"],
            "at": left["completed_at"],
            "finished": left["finished"],
            "conclusion": conclusion,
        }))
        .map_err(|error| error.to_string())?;
        writes.push(Write { order: item.clone(), item, value: Some(body), objects });
    }
    let written = writes.len();
    if written > 0 {
        Writer::<ConversationSentinelRun>::new(library, &scope.key, &stream)
            .map_err(error)?
            .in_holding(holding)
            .write_all(writes, leases, false, kept.signer())
            .map_err(error)?;
    }
    if next != from {
        let at = serde_json::json!({ "holding": holding, "next": next });
        library.keep_checkpoint(&standing, serde_json::to_vec(&at).map_err(|error| error.to_string())?).map_err(error)?;
    }
    Ok(written)
}

/// Whether this machine writes the stream `stream` of `scope`: held here and
/// ordered by none other.
fn writable(library: &Library, scope: &str, stream: &str, device: &[u8; 32]) -> Result<Option<i64>, String> {
    let error = |error: fastctx_content::Error| error.to_string();
    let Some(holding) = library.holding(scope).map_err(error)? else { return Ok(None) };
    if library.orderer(scope, stream).map_err(error)?.is_some_and(|orderer| orderer != *device) {
        return Ok(None);
    }
    Ok(Some(holding))
}
