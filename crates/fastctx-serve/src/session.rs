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

#[cfg(test)]
mod lifetime;

use crate::answer::{self, Answer};

pub struct Session {
    world: WorldDir,
    host: OnceLock<Host>,
    /// Set when the World started this session as a Codex thread.
    resident: Option<String>,
    /// Set when the World started this session as someone's colleague,
    /// which asks for no colleagues of its own.
    colleague: bool,
    /// The residents this session's calls belong to, by Codex thread; calls
    /// with no thread share the entry for `None`.
    residents: tokio::sync::Mutex<HashMap<Option<String>, Arrival>>,
    /// Who the calls of each thread were, kept when the connection to the
    /// node breaks, so the session arrives again as the same resident.
    were: Mutex<HashMap<Option<String>, ResidentId>>,
    /// Starting the node is done by one call at a time.
    starting: tokio::sync::Mutex<()>,
    /// The World's write commands refuse in this session's shell.
    writes_off: bool,
}

struct Arrival {
    whereabouts: Whereabouts,
    client: Client,
}

impl Session {
    pub fn new(world: WorldDir, writes_off: bool) -> anyhow::Result<Self> {
        // Set, as `<id>@<machine>`, when the World started this session.
        let resident = match std::env::var("FASTCTX_RESIDENT") {
            Ok(address) => Some(address),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        let colleague = std::env::var_os("FASTCTX_COLLEAGUE").is_some_and(|value| value == "1");
        Ok(Self {
            world,
            host: OnceLock::new(),
            resident,
            colleague,
            residents: tokio::sync::Mutex::new(HashMap::new()),
            were: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
            writes_off,
        })
    }

    /// Records who the session's host is, from the MCP handshake.
    pub fn introduce(&self, client_info: &Value) {
        let name = client_info.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        let version = client_info.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
        let _ = self.host.set(Host { agent: agent_of(&name), client: name, version });
    }

    pub fn colleague(&self) -> bool {
        self.colleague && self.host.get().is_some_and(|host| host.agent.as_str() != "claude")
    }

    /// Whether the World can start a turn here when something arrives for
    /// this session: in a Codex thread the World started, through its
    /// app-server, and in Claude Code, through the World's plugin. Any other
    /// session, such as Codex the user opened, learns it only with the
    /// answer of its next World tool call.
    pub fn woken(&self) -> bool {
        self.resident.is_some() || self.host.get().is_some_and(|host| host.agent.as_str() == "claude")
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
        // A session listed before `on` replaced it may still name a machine.
        if arguments.get("node").is_some_and(|node| !node.is_null()) {
            return Answer::error(
                "=== run (not run: run has no `node`; write `on MACHINE` in the script to run part of it there) ==="
                    .to_string(),
            );
        }
        // Hosts that do not check the schema pass on what a model wrote from
        // habit, such as Codex's own `workdir`; ignored, the script would run
        // somewhere it did not mean.
        if let Some(unknown) = arguments.as_object().and_then(|arguments| {
            arguments.keys().find(|key| !["command", "cwd", "wait", "node"].contains(&key.as_str()))
        }) {
            return Answer::error(format!(
                "=== run (not run: run has no `{unknown}`; it takes `command`, `cwd` and `wait`) ==="
            ));
        }
        let wait = match arguments.get("wait") {
            None | Some(Value::Null) => None,
            Some(wait) => match wait.as_u64().and_then(|wait| u32::try_from(wait).ok()) {
                Some(wait) => Some(wait),
                None => {
                    return Answer::error(
                        "=== run (not run: `wait` must be a whole number of seconds) ===".to_string(),
                    );
                }
            },
        };
        let whereabouts = match self.whereabouts(meta).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return Answer::error(format!("=== run (not run: {error}) ===")),
        };
        let reply = async {
            let mut client = self.client().await?;
            let call = Call::Run {
                resident: whereabouts.resident.clone(),
                request: RunRequest { script: command, cwd, writes_off: self.writes_off, wait },
                node: None,
            };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let answering = match &reply {
            Ok(Reply::Run(reply)) => Some(reply.job.clone()),
            _ => None,
        };
        let mut answer = match reply {
            Ok(Reply::Run(reply)) => Answer {
                content: present(&reply),
                is_error: false,
                receipt: None,
                source: Some(format!(
                    "{} {}",
                    reply.machine,
                    fastctx_hands::run::present::display_path(&reply.output.log)
                )),
            },
            Ok(other) => Answer::error(format!("=== run (the node answered something else: {other:?}) ===")),
            Err(error) => Answer::error(format!("=== run (not run) ===\n{error}")),
        };
        self.add_waiting_words_about(&whereabouts, answering, &mut answer).await;
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
        let whereabouts = match self.whereabouts(meta).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return Answer::error(format!("=== {tool} (not done: {error}) ===")),
        };
        let reply = async {
            let mut client = self.client().await?;
            let call = Call::Write { resident: Some(whereabouts.resident.clone()), op, node, town: None };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Written { outcome: Ok(result) }) => Answer::text(result.text, false),
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
        let whereabouts = match self.whereabouts(meta).await {
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
                receipt: None,
                source: None,
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
        let whereabouts = match self.whereabouts(meta).await {
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
                    Landed::Uncertain { why } => format!(
                        "acceptance is unknown: {why}; the words remain recorded and will not be submitted again"
                    ),
                    Landed::NewTurn { .. } => "a new turn started with it".to_string(),
                    Landed::JoinedTurn { .. } => "it joins the turn running now".to_string(),
                    Landed::Waiting { why: None, .. } => "waiting until it can be handed over".to_string(),
                    Landed::Waiting { why: Some(why), .. } => {
                        format!("waiting: it could not be handed over now: {why}")
                    }
                    Landed::Sent => format!("sent to {}", to.machine),
                    Landed::Handed { .. } => "handed to its session, which takes it at its next step".to_string(),
                };
                Answer {
                    content: vec![Content::Text(format!(
                        "=== message to {} on {} ({how}) ===\nThe last message of the turn it goes into will be delivered to you.",
                        to.resident, to.machine
                    ))],
                    is_error: false,
                    receipt: None,
                    source: None,
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
        self.add_waiting_words_about(whereabouts, None, answer).await;
    }

    /// What waits for the resident, added to an answer that itself tells of
    /// the job `answering`.
    async fn add_waiting_words_about(
        &self,
        whereabouts: &Whereabouts,
        answering: Option<fastctx_types::JobId>,
        answer: &mut Answer,
    ) {
        let collect = Call::Collect { resident: whereabouts.resident.clone(), answering };
        let mut client = match self.client().await {
            Ok(client) => client,
            Err(error) => {
                answer.follow(format!("=== words for you could not be read: {error} ==="));
                return;
            }
        };
        let collected = client.call(collect).await.map_err(|error| error.to_string());
        match collected {
            Ok(Reply::Notices { notices, notes }) => {
                let messages = notices.into_iter().map(|delivery| (delivery.id, told::told(&delivery.notice)));
                let ids = match answer::assemble(&self.world, answer, notes, messages) {
                    Ok(ids) => ids,
                    Err(error) => {
                        answer.follow(format!("=== could not prepare words for you: {error} ==="));
                        return;
                    }
                };
                // They go out with this answer; until the node hears so, no
                // other answer hands them over, and if it never does, a later
                // one does.
                answer.receipt = Some((client, whereabouts.resident.clone(), ids));
            }
            Ok(other) => answer.follow(format!("=== words for you could not be read: the node answered {other:?} ===")),
            Err(error) => answer.follow(format!("=== words for you could not be read: {error} ===")),
        }
    }

    pub fn bound(&self, answer: &mut Answer) -> std::io::Result<()> {
        answer.bound(&self.world)
    }

    /// Claude proves each call independently; other hosts keep their
    /// connection or thread identity until the node connection closes.
    async fn whereabouts(self: &Arc<Self>, meta: Option<&Value>) -> Result<Whereabouts, String> {
        let host = self.host.get().cloned().ok_or("the session did not introduce itself with `initialize`")?;
        let tool_use_id = meta.and_then(|meta| meta.get("claudecode/toolUseId"));
        if host.agent.as_str() == "claude" || tool_use_id.is_some() {
            let tool_use_id = tool_use_id.and_then(Value::as_str).filter(|id| !id.is_empty())
                .ok_or("Claude did not supply a nonempty _meta[\"claudecode/toolUseId\"]; this call cannot be paired with its mod session")?;
            let hints = Hints { tool_use_id: Some(tool_use_id.to_string()), ..Hints::default() };
            let mut client = self.client().await?;
            return match client.call(Call::Arrive { host, hints }).await {
                Ok(Reply::Arrived(whereabouts)) => Ok(whereabouts),
                Ok(other) => Err(format!("the node answered something else: {other:?}")),
                Err(CallError::Failed(failure)) => Err(failure.message),
                Err(error) => Err(error.to_string()),
            };
        }
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        let session = self.clone();
        // A tool cancellation does not end its session's arrival connection.
        // Finish this read-only probe/registration even if that caller leaves.
        tokio::spawn(async move { session.arrival(host, thread).await })
            .await
            .map_err(|error| format!("checking the session's arrival: {error}"))?
    }

    async fn arrival(self: &Arc<Self>, host: Host, thread: Option<String>) -> Result<Whereabouts, String> {
        let mut residents = self.residents.lock().await;
        if let Some(mut known) = residents.remove(&thread) {
            // Check the arrival connection itself before trusting presence. A detached
            // EOF watcher can run after this call has already reached a replacement node.
            match known.client.call(Call::Machines).await {
                Ok(Reply::Machines { .. }) => {
                    let whereabouts = known.whereabouts.clone();
                    residents.insert(thread, known);
                    return Ok(whereabouts);
                }
                Err(CallError::Broken(_)) => {}
                Err(error) => return Err(error.to_string()),
                Ok(other) => return Err(format!("the node answered something else: {other:?}")),
            }
        }
        let was = self.were.lock().expect("never poisoned").get(&thread).cloned();
        let resident = self
            .resident
            .as_deref()
            .map(str::parse::<Address>)
            .transpose()
            .map_err(|error| error.to_string())?
            .map(|address| address.resident);
        let hints = Hints { resident, thread: thread.clone(), was, tool_use_id: None };
        let mut client = self.client().await?;
        match client.call(Call::Arrive { host, hints }).await {
            Ok(Reply::Arrived(whereabouts)) => {
                self.were.lock().expect("never poisoned").insert(thread.clone(), whereabouts.resident.clone());
                // The connection the resident arrived on stays open for as
                // long as the session lives; if the node closes it, as when
                // it restarts, the next call arrives again.
                residents.insert(thread, Arrival { whereabouts: whereabouts.clone(), client });
                Ok(whereabouts)
            }
            Ok(other) => Err(format!("the node answered something else: {other:?}")),
            Err(CallError::Failed(failure)) => Err(failure.message),
            Err(error) => Err(error.to_string()),
        }
    }

    /// The machines this session works on, for its tools' descriptions; none
    /// when the node is not running, which listing the tools does not start.
    pub async fn machines(&self) -> Option<crate::tools::Machines> {
        let host = self.host.get().cloned()?;
        let mut client = Client::connect(&self.world, Peer::Session { host }).await.ok()?;
        match client.call(Call::Machines).await {
            Ok(Reply::Machines { here, others }) => Some(crate::tools::Machines {
                here: here.to_string(),
                others: others.iter().map(ToString::to_string).collect(),
            }),
            _ => None,
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
