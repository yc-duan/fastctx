//! The resident behind one MCP session.
//!
//! A session becomes a resident when its MCP handshake completes: that is when the host has
//! said what it is (`clientInfo`), and the session's environment says whether the World
//! launched it (`FASTCTX_RESIDENT`). The slot lives as long as the session's server and
//! detaches when the last clone of it is dropped.

use super::core::Town;
use crate::session::SessionEnvironment;
use parking_lot::Mutex;
use std::sync::Arc;

/// Names a resident the World launched; set in the environment of its MCP server.
pub(crate) const RESIDENT_VARIABLE: &str = "FASTCTX_RESIDENT";
/// Present in the MCP server environment of a colleague, which may not ask colleagues itself.
pub(crate) const COLLEAGUE_VARIABLE: &str = "FASTCTX_COLLEAGUE";
/// The entry point Claude Code reports for its session (`cli`, `claude-vscode`, …).
const CLAUDE_ENTRYPOINT_VARIABLE: &str = "CLAUDE_CODE_ENTRYPOINT";

#[derive(Debug)]
pub(crate) struct ResidentSlot {
    town: Arc<Town>,
    id: Mutex<Option<String>>,
}

impl ResidentSlot {
    pub(crate) fn new(town: Arc<Town>) -> Arc<Self> {
        Arc::new(Self { town, id: Mutex::new(None) })
    }

    pub(crate) fn town(&self) -> &Arc<Town> {
        &self.town
    }

    /// The resident id once the handshake completed.
    pub(crate) fn id(&self) -> Option<String> {
        self.id.lock().clone()
    }

    /// Registers the session; called once its handshake completed.
    pub(crate) fn attach(&self, environment: &SessionEnvironment, agent: &str, agent_version: &str) {
        if self.id.lock().is_some() {
            return;
        }
        let named = variable(environment, RESIDENT_VARIABLE);
        let surface = if named.is_some() {
            "world".to_string()
        } else {
            variable(environment, CLAUDE_ENTRYPOINT_VARIABLE).unwrap_or_else(|| "unknown".to_string())
        };
        let inbox = variable(environment, super::claude::INBOX_ADDRESS_VARIABLE);
        match self.town.attach(agent, Some(agent_version), named.as_deref(), &surface, inbox.as_deref()) {
            Ok(id) => *self.id.lock() = Some(id),
            Err(error) => self.town.log(format!("cannot register a session of {agent}: {error}")),
        }
    }

    /// Appends what waits for this resident to a tool result, each delivery as its own block,
    /// for hosts that offer the World no other way to reach it.
    pub(crate) fn piggyback(&self, result: &mut rmcp::model::CallToolResult) {
        let Some(id) = self.id() else { return };
        for delivery in self.town.take_waiting(&id) {
            result.content.push(rmcp::model::ContentBlock::text(delivery.text));
        }
    }

    /// Whether this session belongs to a colleague, which has no colleague tools.
    pub(crate) fn is_colleague(environment: &SessionEnvironment) -> bool {
        variable(environment, COLLEAGUE_VARIABLE).is_some()
    }
}

impl Drop for ResidentSlot {
    fn drop(&mut self) {
        if let Some(id) = self.id.get_mut().take() {
            self.town.detach(&id);
        }
    }
}

/// One variable of a session's environment, compared the way the platform compares names.
pub(crate) fn variable(environment: &SessionEnvironment, name: &str) -> Option<String> {
    environment
        .variables()
        .iter()
        .find(|(key, _)| crate::session::environment_name_eq(key, name))
        .and_then(|(_, value)| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
