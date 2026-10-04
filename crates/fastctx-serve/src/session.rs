//! One agent session: who it is in the World, and its calls to the node.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fastctx_client::{CallError, Client};
use fastctx_hands::run::{Content, present};
use fastctx_hands::told;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Hints, Host, Landed, Whereabouts};
use fastctx_types::run::RunRequest;
use fastctx_types::write::{ApplyPatchRequest, EditRequest, WriteOp, WriteRequest};
use fastctx_types::{Address, AgentName, Call, MachineName, Reply, ResidentId};
use serde_json::Value;

pub struct Session {
    world: WorldDir,
    host: OnceLock<Host>,
    /// Set when the World started this session.
    resident: Option<ResidentId>,
    /// Set when the World started this session as someone's colleague,
    /// which asks for no colleagues of its own.
    pub colleague: bool,
    /// The residents this session's calls belong to, by Codex thread; calls
    /// with no thread share the entry for `None`.
    residents: Mutex<HashMap<Option<String>, Whereabouts>>,
    /// Who the calls of each thread were, kept when the connection to the
    /// node breaks, so the session arrives again as the same resident.
    were: Mutex<HashMap<Option<String>, ResidentId>>,
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
        // Set, as `<id>@<machine>`, when the World started this session.
        let resident = match std::env::var("FASTCTX_RESIDENT") {
            Ok(address) => Some(address.parse::<Address>()?.resident),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        let colleague = std::env::var_os("FASTCTX_COLLEAGUE").is_some_and(|value| value == "1");
        Ok(Self {
            world,
            host: OnceLock::new(),
            resident,
            colleague,
            residents: Mutex::new(HashMap::new()),
            were: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
        })
    }

    /// Records who the session's host is, from the MCP handshake.
    pub fn introduce(&self, client_info: &Value) {
        let name = client_info.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let version = client_info.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
        let _ = self.host.set(Host { agent: agent_of(&name), client: name, version });
    }

    pub async fn run(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let command = match arguments.get("command") {
            Some(Value::String(command)) => command.clone(),
            _ => {
                return Answer::error(
                    "=== run (not run: `command` must be a string holding the script) ===".to_string(),
                );
            }
        };
        let cwd = match arguments.get("cwd") {
            None | Some(Value::Null) => None,
            Some(Value::String(cwd)) => Some(cwd.clone()),
            Some(_) => {
                return Answer::error("=== run (not run: `cwd` must be a string) ===".to_string());
            }
        };
        let node = match arguments.get("node") {
            None | Some(Value::Null) => None,
            Some(Value::String(node)) => match MachineName::try_from(node.as_str()) {
                Ok(node) => Some(node),
                Err(error) => return Answer::error(format!("=== run (not run: {error}) ===")),
            },
            Some(_) => {
                return Answer::error("=== run (not run: `node` must be a string naming a machine) ===".to_string());
            }
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
                request: RunRequest { script: command, cwd },
                node,
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

    /// One of the writing tools: `edit`, `write` or `apply_patch`.
    pub async fn write(self: &Arc<Self>, tool: &str, arguments: &Value, meta: Option<&Value>) -> Answer {
        let op = match write_op(tool, arguments) {
            Ok(op) => op,
            Err(error) => return Answer::error(format!("=== {tool} (not done: {error}) ===")),
        };
        let node = match arguments.get("node") {
            None | Some(Value::Null) => None,
            Some(Value::String(node)) => match MachineName::try_from(node.as_str()) {
                Ok(node) => Some(node),
                Err(error) => return Answer::error(format!("=== {tool} (not done: {error}) ===")),
            },
            Some(_) => {
                return Answer::error(format!("=== {tool} (not done: `node` must be a string naming a machine) ==="));
            }
        };
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        let whereabouts = match self.whereabouts(thread).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return Answer::error(format!("=== {tool} (not done: {error}) ===")),
        };
        let reply = async {
            let mut client = self.client().await?;
            let call = Call::Write { resident: whereabouts.resident.clone(), op, node };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Written { outcome: Ok(result) }) => {
                Answer { content: vec![Content::Text(result.text)], is_error: false }
            }
            Ok(Reply::Written { outcome: Err(error) }) => Answer::error(error.to_string()),
            Ok(other) => Answer::error(format!("=== {tool} (the node answered something else: {other:?}) ===")),
            Err(error) => Answer::error(format!("=== {tool} (not done: {error}) ===")),
        };
        self.add_waiting_words(&whereabouts, &mut answer).await;
        answer
    }

    /// `ask_colleague`: a colleague for this session's resident.
    pub async fn ask(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let failed = |error: String| Answer::error(format!("=== ask_colleague (no colleague asked for: {error}) ==="));
        let (prompt, label) = match (text_argument(arguments, "prompt"), text_argument(arguments, "description")) {
            (Ok(prompt), Ok(label)) => (prompt, label),
            (Err(error), _) | (_, Err(error)) => return failed(error),
        };
        let node = match machine_argument(arguments) {
            Ok(node) => node,
            Err(error) => return failed(error),
        };
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        let whereabouts = match self.whereabouts(thread).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return failed(error),
        };
        let reply = async {
            let mut client = self.client().await?;
            let call = Call::Ask { resident: whereabouts.resident.clone(), label, prompt, node };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Asked { colleague }) => Answer {
                content: vec![Content::Text(format!(
                    "=== colleague {} on {} (started) ===\nIts last message of each turn will be delivered to you. See what it is doing: fastctx w {}",
                    colleague.resident, colleague.machine, colleague.resident
                ))],
                is_error: false,
            },
            Ok(other) => Answer::error(format!("=== ask_colleague (the node answered something else: {other:?}) ===")),
            Err(error) => failed(error),
        };
        self.add_waiting_words(&whereabouts, &mut answer).await;
        answer
    }

    /// `message_colleague`: words from this session's resident to another.
    pub async fn message(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let failed = |error: String| Answer::error(format!("=== message_colleague (not sent: {error}) ==="));
        let (to, message) = match (text_argument(arguments, "to"), text_argument(arguments, "message")) {
            (Ok(to), Ok(message)) => (to, message),
            (Err(error), _) | (_, Err(error)) => return failed(error),
        };
        let (to, machine) = match to.parse::<Address>() {
            Ok(address) => (address.resident, Some(address.machine)),
            Err(_) => match ResidentId::try_from(to.as_str()) {
                Ok(id) => (id, None),
                Err(error) => return failed(error.to_string()),
            },
        };
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        let whereabouts = match self.whereabouts(thread).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return failed(error),
        };
        let reply = async {
            let mut client = self.client().await?;
            let call = Call::Message { resident: whereabouts.resident.clone(), to, machine, text: message };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Spoken { to, landed }) => {
                let how = match landed {
                    Landed::NewTurn { .. } => "a new turn started with it".to_string(),
                    Landed::JoinedTurn { .. } => "it joins the turn running now".to_string(),
                    Landed::Waiting { why: None, .. } => "waiting until it can be handed over".to_string(),
                    Landed::Waiting { why: Some(why), .. } => {
                        format!("waiting: it could not be handed over now: {why}")
                    }
                    Landed::Sent => format!("sent to {}", to.machine),
                    Landed::Handed => "handed to its session, which takes it at its next step".to_string(),
                };
                Answer {
                    content: vec![Content::Text(format!(
                        "=== message to {} on {} ({how}) ===\nThe last message of the turn it goes into will be delivered to you.",
                        to.resident, to.machine
                    ))],
                    is_error: false,
                }
            }
            Ok(other) => {
                Answer::error(format!("=== message_colleague (the node answered something else: {other:?}) ==="))
            }
            Err(error) => failed(error),
        };
        self.add_waiting_words(&whereabouts, &mut answer).await;
        answer
    }

    /// What the resident is told of where it lives, and words waiting for
    /// it, ride along with its answer, each in a block of its own
    /// (`world-colleagues.md`, 话怎样送到居民眼前; `world-projects.md`).
    async fn add_waiting_words(&self, whereabouts: &Whereabouts, answer: &mut Answer) {
        let collect = Call::Collect { resident: whereabouts.resident.clone() };
        let collected = match self.client().await {
            Ok(mut client) => client.call(collect).await.map_err(|error| error.to_string()),
            Err(error) => Err(error),
        };
        match collected {
            Ok(Reply::Notices { notices, notes }) => {
                answer.content.extend(notes.into_iter().map(Content::Text));
                answer.content.extend(notices.iter().map(|delivery| Content::Text(told::told(&delivery.notice))));
                // They go out with this answer; until the node hears so, no
                // other answer hands them over, and if it never does, a later
                // one does.
                if !notices.is_empty() {
                    let ids = notices.into_iter().map(|delivery| delivery.id).collect();
                    let taken = Call::Taken { resident: whereabouts.resident.clone(), ids };
                    if let Ok(mut client) = self.client().await {
                        let _ = client.call(taken).await;
                    }
                }
            }
            Ok(other) => answer
                .content
                .push(Content::Text(format!("=== words for you could not be read: the node answered {other:?} ==="))),
            Err(error) => {
                answer.content.push(Content::Text(format!("=== words for you could not be read: {error} ===")))
            }
        }
    }

    /// The resident the calls of `thread` belong to, found or made by the
    /// node on first use.
    async fn whereabouts(self: &Arc<Self>, thread: Option<String>) -> Result<Whereabouts, String> {
        if let Some(known) = self.residents.lock().expect("never poisoned").get(&thread) {
            return Ok(known.clone());
        }
        let host = self.host.get().cloned().ok_or("the session did not introduce itself with `initialize`")?;
        let was = self.were.lock().expect("never poisoned").get(&thread).cloned();
        // A Claude session's inbox address pairs this connection with what
        // the World's mod reports of the session; its token is never read.
        let inbox = std::env::var("CLAUDE_CODE_MESSAGING_SOCKET").ok().filter(|address| !address.is_empty());
        let hints = Hints { resident: self.resident.clone(), thread: thread.clone(), was, inbox };
        let mut client = self.client().await?;
        match client.call(Call::Arrive { host, hints }).await {
            Ok(Reply::Arrived(whereabouts)) => {
                self.residents.lock().expect("never poisoned").insert(thread.clone(), whereabouts.clone());
                self.were.lock().expect("never poisoned").insert(thread.clone(), whereabouts.resident.clone());
                // The connection the resident arrived on stays open for as
                // long as the session lives; if the node closes it, as when
                // it restarts, the next call arrives again.
                let session = self.clone();
                tokio::spawn(async move {
                    client.closed().await;
                    session.residents.lock().expect("never poisoned").remove(&thread);
                });
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

/// The write a tool call asks for, from its arguments.
fn write_op(tool: &str, arguments: &Value) -> Result<WriteOp, String> {
    let text = |name: &str| match arguments.get(name) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(format!("`{name}` must be a string")),
        None => Err(format!("`{name}` is missing")),
    };
    Ok(match tool {
        "edit" => WriteOp::Edit(EditRequest {
            file_path: text("file_path")?.into(),
            old_string: text("old_string")?,
            new_string: text("new_string")?,
            replace_all: match arguments.get("replace_all") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(all)) => *all,
                Some(_) => return Err("`replace_all` must be true or false".to_string()),
            },
        }),
        "write" => WriteOp::Write(WriteRequest { file_path: text("file_path")?.into(), content: text("content")? }),
        "apply_patch" => WriteOp::ApplyPatch(ApplyPatchRequest { input: text("input")? }),
        other => return Err(format!("{other} is not a writing tool")),
    })
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

/// A string argument.
fn text_argument(arguments: &Value, name: &str) -> Result<String, String> {
    match arguments.get(name) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(format!("`{name}` must be a string")),
        None => Err(format!("`{name}` is missing")),
    }
}

/// The `node` argument, when given.
fn machine_argument(arguments: &Value) -> Result<Option<MachineName>, String> {
    match arguments.get("node") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(node)) => MachineName::try_from(node.as_str()).map(Some).map_err(|error| error.to_string()),
        Some(_) => Err("`node` must be a string naming a machine".to_string()),
    }
}
