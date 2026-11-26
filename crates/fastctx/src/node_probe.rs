//! What a node's service status says of the node itself, beside what the
//! service manager says: whether a node answers at the World's endpoint,
//! which process it is and how it runs; or else where reaching it fails, and
//! which process holds the World meanwhile.

use std::path::Path;
use std::time::Duration;

use fastctx_client::renewal::Probe;
use fastctx_platform::FileLock;
use fastctx_types::ProcessIdentity;
use fastctx_types::layout::WorldDir;
use serde_json::{Value, json};

/// How long the node is given to answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// The node of the World under `root`, as it answers now.
pub(crate) fn probe(root: &Path) -> Value {
    let world = WorldDir::under(root);
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return json!({ "state": "unknown", "error": error.to_string() }),
    };
    runtime.block_on(async {
        match tokio::time::timeout(ANSWER_WITHIN, fastctx_client::renewal::probe(&world)).await {
            Ok(Ok(Probe::Running(node))) => json!({
                "state": if node.yielding { "handing over" } else { "serving" },
                "process": node.process.pid,
                "version": node.build.version,
                "supervised": node.supervised,
                "bridge": node.bridge,
            }),
            Ok(Ok(Probe::Legacy { mismatch, .. })) => json!({
                "state": "serving",
                "version": mismatch.node_version,
                "earlier_protocol": mismatch.node_protocol,
            }),
            Ok(Ok(Probe::Absent)) => json!({ "state": "not listening", "holder": holder(&world) }),
            Ok(Err(error)) => json!({ "state": "not answering", "error": error.to_string(), "holder": holder(&world) }),
            Err(_) => json!({
                "state": "not answering",
                "error": format!("the node took the connection and answered nothing for {} seconds", ANSWER_WITHIN.as_secs()),
                "holder": holder(&world),
            }),
        }
    })
}

/// The process holding the World's lock, as its holder record names it.
fn holder(world: &WorldDir) -> Value {
    match FileLock::try_acquire(&world.node_lock()) {
        Ok(Some(_free)) => return Value::Null,
        Ok(None) => {}
        Err(error) => return json!({ "error": error.to_string() }),
    }
    let recorded = std::fs::read(world.node_holder())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ProcessIdentity>(&bytes).ok());
    match recorded {
        Some(holder) => json!({
            "process": holder.pid,
            "alive": fastctx_platform::is_alive(&holder).ok(),
        }),
        None => json!({ "process": null }),
    }
}
