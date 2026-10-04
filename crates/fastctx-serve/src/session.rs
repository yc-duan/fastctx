//! One agent session: who it is in the World, and its calls to the node.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use fastctx_client::{CallError, Client};
use fastctx_hands::run::{Content, present, present_notice};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Hints, Host, Whereabouts};
use fastctx_types::run::RunRequest;
use fastctx_types::{AgentName, Call, Reply, ResidentId};
use serde_json::Value;

pub struct Session {
    world: WorldDir,
    host: OnceLock<Host>,
    /// Set when the World started this session.
    resident: Option<ResidentId>,
    /// The residents this session's calls belong to, by Codex thread; calls
    /// with no thread share the entry for `None`.
    residents: Mutex<HashMap<Option<String>, Whereabouts>>,
    /// Starting the node is done by one call at a time.
    starting: tokio::sync::Mutex<()>,
}

/// What a tool call answers.
pub struct Answer {
    pub content: Vec<Content>,
    pub is_error: bool,
}

impl Answer {
    fn error(text: String) -> Self {
        Self { content: vec![Content::Text(text)], is_error: true }
    }
}

impl Session {
    pub fn new(world: WorldDir) -> anyhow::Result<Self> {
        let resident = match std::env::var("FASTCTX_RESIDENT") {
            Ok(id) => Some(ResidentId::try_from(id)?),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            world,
            host: OnceLock::new(),
            resident,
            residents: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
        })
    }

    /// Records who the session's host is, from the MCP handshake.
    pub fn introduce(&self, client_info: &Value) {
        let name = client_info.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let version = client_info.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
        let _ = self.host.set(Host { agent: agent_of(&name), client: name, version });
    }

    pub async fn run(&self, arguments: &Value, meta: Option<&Value>) -> Answer {
        let command = match arguments.get("command") {
            Some(Value::String(command)) => command.clone(),
            _ => return Answer::error("=== run (not run: `command` must be a string holding the script) ===".to_string()),
        };
        let cwd = match arguments.get("cwd") {
            None | Some(Value::Null) => None,
            Some(Value::String(cwd)) => Some(cwd.clone()),
            Some(_) => return Answer::error("=== run (not run: `cwd` must be a string) ===".to_string()),
        };
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        let whereabouts = match self.whereabouts(thread).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return Answer::error(format!("=== run (not run: {error}) ===")),
        };
        let reply = async {
            let mut client = self.client().await?;
            let call = Call::Run {
                resident: whereabouts.resident.clone(),
                town: whereabouts.town.clone(),
                request: RunRequest { script: command, cwd },
            };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Run(reply)) => Answer { content: present(&reply), is_error: false },
            Ok(other) => Answer::error(format!("=== run (the node answered something else: {other:?}) ===")),
            Err(error) => Answer::error(format!("=== run (not run: {error}) ===")),
        };
        self.add_waiting_words(&whereabouts, &mut answer).await;
        answer
    }

    /// Words waiting for the resident ride along with its answer, each in a
    /// block of its own (`world-colleagues.md`, 话怎样送到居民眼前).
    async fn add_waiting_words(&self, whereabouts: &Whereabouts, answer: &mut Answer) {
        let collect = Call::Collect { resident: whereabouts.resident.clone(), town: whereabouts.town.clone() };
        let collected = match self.client().await {
            Ok(mut client) => client.call(collect).await.map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };
        match collected {
            Ok(Reply::Notices { notices }) => answer.content.extend(notices.iter().map(present_notice)),
            Ok(other) => answer.content.push(Content::Text(format!("=== words for you could not be read: the node answered {other:?} ==="))),
            Err(error) => answer.content.push(Content::Text(format!("=== words for you could not be read: {error} ==="))),
        }
    }

    /// The resident the calls of `thread` belong to, found or made by the
    /// node on first use.
    async fn whereabouts(&self, thread: Option<String>) -> Result<Whereabouts, String> {
        if let Some(known) = self.residents.lock().expect("never poisoned").get(&thread) {
            return Ok(known.clone());
        }
        let host = self.host.get().cloned().ok_or("the session did not introduce itself with `initialize`")?;
        let hints = Hints { resident: self.resident.clone(), thread: thread.clone() };
        let mut client = self.client().await?;
        match client.call(Call::Arrive { host, hints }).await {
            Ok(Reply::Arrived(whereabouts)) => {
                self.residents.lock().expect("never poisoned").insert(thread, whereabouts.clone());
                Ok(whereabouts)
            }
            Ok(other) => Err(format!("the node answered something else: {other:?}")),
            Err(CallError::Failed(failure)) => Err(failure.message),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn client(&self) -> Result<Client, String> {
        let host = self.host.get().cloned().ok_or("the session did not introduce itself with `initialize`")?;
        let _starting = self.starting.lock().await;
        Client::connect_or_start(&self.world, Peer::Session { host }).await.map_err(|error| error.to_string())
    }
}

/// The agent a client name belongs to: `codex` and `claude` by the names
/// their hosts send, otherwise the name itself made into a word.
fn agent_of(client: &str) -> AgentName {
    let lower = client.to_lowercase();
    let word = if lower.contains("codex") {
        "codex".to_string()
    } else if lower.contains("claude") {
        "claude".to_string()
    } else {
        let mut word: String = lower
            .chars()
            .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        word.truncate(16);
        word.trim_end_matches('-').to_string()
    };
    AgentName::try_from(word).unwrap_or_else(|_| AgentName::try_from("agent").expect("a valid agent name"))
}
