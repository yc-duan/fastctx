//! A borrowed app-server cannot be replaced underneath its caller. This
//! includes the interval before a new thread or turn is known to the host.

use std::ops::Deref;
use std::sync::Arc;

use serde_json::json;

use super::{CodexHost, server::AppServer};

pub(super) struct ServerUse {
    server: Arc<AppServer>,
    host: Arc<CodexHost>,
}

impl ServerUse {
    /// The caller holds host state while selecting this generation and
    /// registering its use, so replacement cannot race the handoff.
    pub(super) fn new(server: Arc<AppServer>, host: Arc<CodexHost>) -> Self {
        Self { server, host }
    }

    /// Preserve a new, unsectioned thread before handing out its ID.
    ///
    /// Codex 0.160 defers creating a paginated thread's rollout. Neither
    /// thread/start, name/set nor read(includeTurns:false) persists it.
    /// The stable section/move RPC materializes and flushes the live thread
    /// before updating its section. Keeping its initial null section leaves
    /// it ungrouped and performs no model request. Failure must prevent a
    /// successful handoff: a subsequent process replacement would lose it.
    pub(super) async fn persist_new_thread(&self, thread: &str) -> Result<(), String> {
        self.request("thread/section/move", json!({"threadId": thread, "sectionId": null}))
            .await
            .map_err(|error| format!("preserving the new Codex thread {thread}: {error}"))?;
        Ok(())
    }
}

impl Deref for ServerUse {
    type Target = Arc<AppServer>;

    fn deref(&self) -> &Self::Target {
        &self.server
    }
}

impl Drop for ServerUse {
    fn drop(&mut self) {
        self.host.lock().server_users -= 1;
    }
}
