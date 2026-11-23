//! The logs that earlier releases wrote into the asset library: this
//! machine's own log in the stream `node` of its body's scope `b/<body>/log`,
//! each MCP command's log in the stream `mcp` of its account's scope on this
//! machine, and the hub's log in `h/<hub>/log`. Nothing read them, so none is
//! written any more; each machine's log stays in its file. What is held here
//! of them goes a week after it was written, as before.

use crate::Node;
use crate::assets::Proposal;
use fastctx_content::v2::{HubLog, KEPT_MS, Kind as _, MachineLog};
use fastctx_net::ids::HubId;
use fastctx_platform::log_tail;
use fastctx_types::assets::Readers;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often what is held of the logs is looked at for pieces a week old.
const EVERY: Duration = Duration::from_secs(10 * 60);

/// Starts letting the pieces of the logs held here go as they grow old.
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
            if let Err(error) = fastctx_platform::task::blocking(move || prune(&working)).await {
                node.log.line(format_args!("letting the oldest pieces of the logs here go: {error}"));
            }
        }
    });
}

/// The scope `readers` and `name` make on `hub`: proposed here while it is
/// not held, and the key with whether it is held.
pub(super) fn proposed(node: &Node, hub: &HubId, readers: Readers, name: &str) -> Result<Option<String>, String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    let library = &kept.library;
    let key = format!("{}{name}", readers.key_prefix(hub.as_str()));
    let error = |error: fastctx_content::Error| error.to_string();
    if library.holding(&key).map_err(error)?.is_some() {
        return Ok(Some(key));
    }
    let Some(network) = crate::assets::membership::network(node, hub) else { return Ok(None) };
    // Proposed with its kind's classes, as the hub describes it (AS09).
    let classes = fastctx_types::assets::classes_for(&readers, &key);
    let proposal = Proposal { hub: hub.to_string(), readers, classes, born: None };
    let proposal = serde_json::to_vec(&proposal).map_err(|error| error.to_string())?;
    library.propose_on(&network, &key, proposal).map_err(error)?;
    Ok(None)
}

/// Lets the pieces of each log held here older than a week go: only what
/// the hub took of this machine's own.
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
        // The pieces whose last line is older than a week, in the order they
        // were written, up to the first that is not: what goes is a prefix
        // of the stream, and none of it is of the week (M14, PP11, N13).
        let cutoff = jiff::Timestamp::now().as_millisecond() - KEPT_MS;
        let Some(at) = library.view_revision(&scope, stream).map_err(error)? else { continue };
        let mut from = at & !((1u64 << 40) - 1);
        let mut through: Option<u64> = None;
        'pieces: for _ in 0..8 {
            let Some(read) = library.view_changes(&scope, stream, from, 500, u32::MAX).map_err(error)? else { break };
            for change in &read.changes {
                let fastctx_content::v2::ViewChange::Is(entry) = change else { continue };
                let last = serde_json::from_slice::<serde_json::Value>(&entry.body)
                    .ok()
                    .and_then(|body| body.get("last").and_then(serde_json::Value::as_i64))
                    .or_else(|| log_tail::order_time(&entry.order))
                    .unwrap_or(i64::MAX);
                if last >= cutoff {
                    break 'pieces;
                }
                through = Some(entry.revision);
            }
            if !read.more {
                break;
            }
            from = read.through;
        }
        let Some(through) = through else { continue };
        let own = library.orderer(&scope, stream).map_err(error)?.is_some_and(|orderer| orderer == device);
        library.prune(&scope, stream, through, own).map_err(error)?;
    }
    Ok(())
}
