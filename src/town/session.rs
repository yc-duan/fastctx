//! The resident behind one MCP session.
//!
//! A session whose environment names it becomes a resident in its handshake: the World named
//! the residents it launched (`FASTCTX_RESIDENT`), and Claude Code names each session's inbox.
//! Any other session becomes one with its first tool call, under the thread its host names in
//! that call when it names one: Codex starts the server of one thread more than once (a new
//! working directory, a thread loaded again), and a status query starts one that never calls a
//! tool, so the thread, not the connection, is the resident. The slot lives as long as the
//! session's server and detaches when the last clone of it is dropped.

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
    /// What the host said it is, kept for the first call of a session its environment does not
    /// name; held while that call registers it, so calls made at once register it once.
    host: Mutex<Option<Host>>,
    /// Whether the resident was already told who and where it is.
    introduced: std::sync::atomic::AtomicBool,
}

/// What a host said about itself in the handshake.
#[derive(Debug)]
struct Host {
    agent: String,
    version: String,
    surface: String,
}

impl ResidentSlot {
    pub(crate) fn new(town: Arc<Town>) -> Arc<Self> {
        Arc::new(Self {
            town,
            id: Mutex::new(None),
            host: Mutex::new(None),
            introduced: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// On the first World tool response of the session, one more head line says who the
    /// resident is and where (`specs/world-projects.md`, "世界请居民进来").
    pub(crate) fn introduce(&self, result: &mut rmcp::model::CallToolResult) {
        if self
            .introduced
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let Some(id) = self.id() else { return };
        let Some(town) = self.town.town_of(&id) else {
            return;
        };
        let machine = self.town.machine();
        let line = match town {
            Some(town) => format!(
                "=== you are {id} on {machine}, in the town {town}; fastctx w shows who else is here ==="
            ),
            None => format!(
                "=== you are {id} on {machine}, outside every town; fastctx w shows the towns, and fastctx project enter <town> enters one when the user tells you which project to work on ==="
            ),
        };
        // The head note stays the first line: it names the machine the call happened on.
        match result.content.first_mut() {
            Some(rmcp::model::ContentBlock::Text(text)) => match text.text.split_once('\n') {
                Some((head, rest)) => text.text = format!("{head}\n{line}\n{rest}"),
                None => text.text = format!("{}\n{line}", text.text),
            },
            _ => result
                .content
                .insert(0, rmcp::model::ContentBlock::text(line)),
        }
    }

    pub(crate) fn town(&self) -> &Arc<Town> {
        &self.town
    }

    /// The resident id once the handshake completed.
    pub(crate) fn id(&self) -> Option<String> {
        self.id.lock().clone()
    }

    /// Takes the handshake of the session; called while its host's `initialize` is answered,
    /// before any call. A session its environment names registers now.
    pub(crate) fn attach(
        &self,
        environment: &SessionEnvironment,
        agent: &str,
        agent_version: &str,
    ) {
        if self.id.lock().is_some() {
            return;
        }
        let named = variable(environment, RESIDENT_VARIABLE);
        let surface = if named.is_some() {
            "world".to_string()
        } else {
            variable(environment, CLAUDE_ENTRYPOINT_VARIABLE)
                .unwrap_or_else(|| "unknown".to_string())
        };
        let inbox = variable(environment, super::claude::INBOX_ADDRESS_VARIABLE);
        let host = Host {
            agent: agent.to_string(),
            version: agent_version.to_string(),
            surface,
        };
        if named.is_none() && inbox.is_none() {
            *self.host.lock() = Some(host);
            return;
        }
        self.register(&host, named.as_deref(), inbox.as_deref(), None);
    }

    /// A session its environment did not name registers with its first call, under `thread`,
    /// the host's own thread of the call when the host names one.
    pub(crate) fn attach_on_call(&self, thread: Option<&str>) {
        let mut host = self.host.lock();
        if self.id.lock().is_some() {
            return;
        }
        if let Some(host) = host.take() {
            self.register(&host, None, None, thread);
        }
    }

    fn register(
        &self,
        host: &Host,
        named: Option<&str>,
        inbox: Option<&str>,
        thread: Option<&str>,
    ) {
        match self.town.attach(
            &host.agent,
            Some(&host.version),
            named,
            &host.surface,
            inbox,
            thread,
        ) {
            Ok(id) => {
                // A resident that comes back is told what ended while it was away, in every place
                // it has records.
                let town = Arc::clone(&self.town);
                let returning = id.clone();
                tokio::spawn(async move {
                    for dir in town.resident_dirs(&returning) {
                        town.owed_ends(&dir, false).await;
                    }
                });
                *self.id.lock() = Some(id);
            }
            Err(error) => self.town.log(format!(
                "cannot register a session of {}: {error}",
                host.agent
            )),
        }
    }

    /// Appends what waits for this resident to a tool result, each delivery as its own block,
    /// for hosts that offer the World no other way to reach it.
    pub(crate) fn piggyback(&self, result: &mut rmcp::model::CallToolResult) {
        let Some(id) = self.id() else { return };
        for delivery in self.town.take_waiting(&id) {
            result
                .content
                .push(rmcp::model::ContentBlock::text(delivery.text));
        }
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
