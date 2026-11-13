//! This machine's logs in the asset library (design §14, §28.21), a look a
//! minute on each network it is on: its own log, read on as it grows, into
//! the stream `node` of its body's scope `b/<body>/log` — its owner and
//! those it is shared with whole read it, a host assisting none — each
//! piece of whole lines named by its file and where it begins; and the log
//! of each MCP command an account runs here, as it stands, into the stream
//! `mcp` of that account's scope on this machine `w/<hub>/<account>/<me>/mcp`,
//! which only the account reads, and only while it can work here. Each scope
//! is proposed here and described by the hub as any other. Of the pieces of
//! a log — this machine's, the hub's, another machine's it holds — a device
//! keeps the latest `KEPT_PIECES`; the hub keeps all, so only what it took
//! goes.

use crate::Node;
use fastctx_content::v2::{HubLog, KEPT_PIECES, Kind as _, LEASE, Library, MachineLog, McpLog, Write, Writer};
use fastctx_net::ids::HubId;
use fastctx_platform::log_tail::{self, Tail};
use crate::assets::Proposal;
use fastctx_types::assets::{Holders, Readers, StreamClass};
use fastctx_types::{AccountName, MachineName};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often the logs are looked at.
const EVERY: Duration = Duration::from_secs(60);
/// The most a piece holds, and a look writes of the machine's log.
const PIECE: usize = 1 << 20;
const A_LOOK: usize = 16 << 20;

/// Starts looking at the logs here every minute.
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
                let me_name = view.me.clone();
                let logins: Vec<AccountName> = me.logins.iter().cloned().collect();
                let home = me.home.clone();
                let body = view.directory.body_of(&view.me);
                drop(view);
                let looked = fastctx_platform::task::blocking(move || {
                    look(&working, &hub, &me_name, body, home, &logins)
                })
                .await;
                if let Err(error) = looked {
                    node.log.line(format_args!("writing the logs here into the asset library: {error}"));
                }
            }
            let working = node.clone();
            if let Err(error) = fastctx_platform::task::blocking(move || prune(&working)).await {
                node.log.line(format_args!("letting the oldest pieces of the logs here go: {error}"));
            }
        }
    });
}

/// The scope `readers` and `name` make on `hub`: proposed here while it is
/// not held, and the key with whether it is held.
pub(super) fn proposed(node: &Node, hub: &HubId, readers: Readers, name: &str, holders: Holders) -> Result<Option<String>, String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let library = &kept.library;
    let key = format!("{}{name}", readers.key_prefix(hub.as_str()));
    let error = |error: fastctx_content::Error| error.to_string();
    if library.holding(&key).map_err(error)?.is_some() {
        return Ok(Some(key));
    }
    let Some(network) = crate::assets::membership::network(node, hub) else { return Ok(None) };
    let proposal = Proposal { hub: hub.to_string(), readers, classes: vec![StreamClass { prefix: String::new(), holders }] };
    let proposal = serde_json::to_vec(&proposal).map_err(|error| error.to_string())?;
    library.propose_on(&network, &key, proposal).map_err(error)?;
    Ok(None)
}

/// One look at the logs here for `hub`.
fn look(
    node: &Arc<Node>,
    hub: &HubId,
    me: &MachineName,
    body: Option<String>,
    home: Option<AccountName>,
    logins: &[AccountName],
) -> Result<(), String> {
    // The machine's own log, in its body's scope: opened by its owner, as
    // one of the accounts it is logged in as.
    if let (Some(body), Some(home)) = (body, home.filter(|home| logins.contains(home)))
        && let Some(scope) = proposed(node, hub, Readers::Body { body, opener: home }, "log", Holders::Logged)?
    {
        own_log(node, &scope)?;
    }
    // The hub's log, read by its super administrators: proposed by a
    // machine one of them is logged in on, written by the hub.
    if let Some(link) = node.link_of(hub)
        && logins.iter().any(|login| link.view().directory.is_super(login))
    {
        proposed(node, hub, Readers::Hub, "log", Holders::Logged)?;
    }
    // Each account's MCP commands' logs, in its scope on this machine.
    let logs = node.world.path().join("mcp").join(hub.as_str()).join("logs");
    let accounts = match fastctx_platform::disk::read_dir(&logs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("looking at {}: {error}", logs.display())),
    };
    for entry in accounts.flatten() {
        let Some(account) = entry.file_name().to_str().and_then(|name| AccountName::try_from(name.to_string()).ok())
        else {
            continue;
        };
        let readers = Readers::Working { account, machine: me.clone() };
        if let Some(scope) = proposed(node, hub, readers, "mcp", Holders::Readers)? {
            mcp_logs(node, &scope, &entry.path())?;
        }
    }
    Ok(())
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

/// Writes what is new of this machine's log into `scope`.
fn own_log(node: &Arc<Node>, scope: &str) -> Result<(), String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(()) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let Some(holding) = writable(library, scope, MachineLog::STREAM, &kept.device())? else { return Ok(()) };
    let standing = format!("logs/{scope}");
    let mut at: Tail = library
        .checkpoint(&standing)
        .map_err(error)?
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(|value| value["holding"].as_i64() == Some(holding))
        .map(|value| Tail {
            file: value["file"].as_str().unwrap_or_default().to_string(),
            offset: value["offset"].as_u64().unwrap_or(0),
        })
        .unwrap_or_default();
    let path = node.world.node_log();
    let (mut writes, mut leases, mut written) = (Vec::new(), Vec::new(), 0);
    while written < A_LOOK {
        let Some(piece) = log_tail::next(&path, &at, PIECE).map_err(|error| error.to_string())? else { break };
        let item = format!("{}/{:016x}", piece.file, piece.offset).into_bytes();
        // Written before a stop that kept no standing: as it was.
        if library.item(scope, MachineLog::STREAM, &item).map_err(error)?.is_none() {
            let order = format!(
                "{:016x}/{}/{:016x}",
                log_tail::first_time(&piece.bytes).unwrap_or(0).max(0),
                piece.file,
                piece.offset
            )
            .into_bytes();
            let lease = library.put(&mut piece.bytes.as_slice(), "machine-log", LEASE).map_err(error)?;
            let body = serde_json::to_vec(
                &serde_json::json!({ "file": piece.file, "offset": piece.offset, "length": piece.bytes.len() }),
            )
            .map_err(|error| error.to_string())?;
            writes.push(Write { item, order, value: Some(body), objects: vec![lease.object] });
            leases.push(lease);
        }
        written += piece.bytes.len();
        at = Tail { offset: piece.offset + piece.bytes.len() as u64, file: piece.file };
    }
    if written == 0 {
        return Ok(());
    }
    if !writes.is_empty() {
        Writer::<MachineLog>::new(library, scope, MachineLog::STREAM)
            .map_err(error)?
            .in_holding(holding)
            .write_all(writes, leases, false, kept.signer())
            .map_err(error)?;
    }
    let standing_now = serde_json::to_vec(&serde_json::json!({ "holding": holding, "file": at.file, "offset": at.offset }))
        .map_err(|error| error.to_string())?;
    library.keep_checkpoint(&standing, standing_now).map_err(error)
}

/// Writes each MCP command's log in `folder` that changed into `scope`, and
/// lets go of those of commands gone.
fn mcp_logs(node: &Arc<Node>, scope: &str, folder: &std::path::Path) -> Result<(), String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(()) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let Some(holding) = writable(library, scope, McpLog::STREAM, &kept.device())? else { return Ok(()) };
    let mut present = std::collections::BTreeSet::new();
    let (mut writes, mut leases) = (Vec::new(), Vec::new());
    for entry in fastctx_platform::disk::read_dir(folder).map_err(|error| error.to_string())?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_suffix(".log")) else {
            continue;
        };
        if name.is_empty() || name.contains('/') {
            continue;
        }
        let bytes = match fastctx_platform::disk::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("reading {}: {error}", path.display())),
        };
        present.insert(name.to_string());
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let item = name.as_bytes().to_vec();
        if library
            .item(scope, McpLog::STREAM, &item)
            .map_err(error)?
            .is_some_and(|entry| entry.objects.first().is_some_and(|object| object.hash == hash))
        {
            continue;
        }
        let lease = library.put(&mut bytes.as_slice(), "mcp-log", LEASE).map_err(error)?;
        let body = serde_json::to_vec(&serde_json::json!({ "name": name, "length": bytes.len() }))
            .map_err(|error| error.to_string())?;
        writes.push(Write { order: item.clone(), item, value: Some(body), objects: vec![lease.object] });
        leases.push(lease);
    }
    for entry in library.page(scope, McpLog::STREAM, None, usize::MAX).map_err(error)? {
        if !present.contains(String::from_utf8_lossy(&entry.item).as_ref()) {
            writes.push(Write { order: entry.order.clone(), item: entry.item, value: None, objects: Vec::new() });
        }
    }
    if writes.is_empty() {
        return Ok(());
    }
    Writer::<McpLog>::new(library, scope, McpLog::STREAM)
        .map_err(error)?
        .in_holding(holding)
        .write_all(writes, leases, false, kept.signer())
        .map_err(error)?;
    Ok(())
}

/// Lets the oldest pieces of each log held here go, the latest
/// `KEPT_PIECES` of each kept: only what the hub took of this machine's own.
fn prune(node: &Arc<Node>) -> Result<(), String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(()) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let device = kept.device();
    for (scope, _, _) in library.live_scopes().map_err(error)? {
        let stream = if scope.starts_with("b/") && scope.ends_with("/log") {
            MachineLog::STREAM
        } else if scope.starts_with("h/") {
            HubLog::STREAM
        } else {
            continue;
        };
        let Some(next) =
            library.streams(&scope).map_err(error)?.into_iter().find(|(name, _)| name == stream).map(|(_, next)| next)
        else {
            continue;
        };
        if next <= KEPT_PIECES {
            continue;
        }
        let own = library.orderer(&scope, stream).map_err(error)?.is_some_and(|orderer| orderer == device);
        library.prune(&scope, stream, next - KEPT_PIECES - 1, own).map_err(error)?;
    }
    Ok(())
}
