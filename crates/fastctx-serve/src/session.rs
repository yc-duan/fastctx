//! One agent session: who it is in the World, and its calls to the node.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fastctx_client::{CallError, Client};
use fastctx_hands::run::Content;
use fastctx_hands::told;
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{Hints, Host, Landed, Whereabouts};
use fastctx_types::run::RunRequest;
use fastctx_types::write::{ApplyPatchRequest, EditRequest, WriteOp, WriteRequest};
use fastctx_types::{Address, AgentName, Call, Reply, ResidentId};
use serde_json::Value;

#[cfg(test)]
mod lifetime;

use crate::answer::{self, Answer};
use crate::protocol::{CLIENT_INFO, VERSION_KEY};

pub struct Session {
    world: WorldDir,
    host: OnceLock<Host>,
    host_process: Option<fastctx_types::ProcessIdentity>,
    /// Set when the World started this session as a Codex thread.
    resident: Option<String>,
    /// The conversation a Claude Code session the World started is, named
    /// by the settings it was started with: known before any call arrives,
    /// so the tools it lists first are already that conversation's.
    conversation: Option<ResidentId>,
    /// The residents this session's calls belong to, by Codex thread; calls
    /// with no thread share the entry for `None`.
    residents: tokio::sync::Mutex<HashMap<Option<String>, Arrival>>,
    /// Who the calls of each thread were, kept when the connection to the
    /// node breaks, so the session arrives again as the same resident.
    were: Mutex<HashMap<Option<String>, ResidentId>>,
    /// Starting the node is done by one call at a time.
    starting: tokio::sync::Mutex<()>,
    /// Emit stderr diagnostics once per connected node incarnation.
    lifetime_reported: Mutex<Option<fastctx_types::ProcessIdentity>>,
    /// The World's write commands refuse in this session's shell.
    writes_off: bool,
    /// The task telling the host its tools changed, for which resident.
    watching: Mutex<Option<(Option<ResidentId>, tokio::task::JoinHandle<()>)>>,
    /// Whether a node was watched before, so a new one is told of.
    watched: std::sync::atomic::AtomicBool,
}

struct Arrival {
    whereabouts: Whereabouts,
    client: Client,
}

impl Session {
    pub fn new(world: WorldDir, writes_off: bool) -> anyhow::Result<Self> {
        // Set, as `<id>@<machine>`, when the World started this session.
        let resident = match std::env::var("FASTCTX_RESIDENT") {
            Ok(address) => {
                address.parse::<Address>().map_err(anyhow::Error::msg)?;
                Some(address)
            }
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        let conversation = match std::env::var("FASTCTX_CONVERSATION") {
            Ok(id) => Some(ResidentId::try_from(id).map_err(anyhow::Error::msg)?),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            world,
            host: OnceLock::new(),
            host_process: fastctx_platform::host_process::parent_identity().ok(),
            resident,
            conversation,
            residents: tokio::sync::Mutex::new(HashMap::new()),
            were: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
            lifetime_reported: Mutex::new(None),
            writes_off,
            watching: Mutex::new(None),
            watched: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Records who the session's host is, from the MCP handshake.
    pub fn introduce(&self, client_info: &Value) {
        let _ = self.host.set(host_info(client_info));
    }

    pub fn initialized(&self) -> bool {
        self.host.get().is_some()
    }

    /// Modern metadata belongs only to this request. Missing clientInfo is
    /// an unknown agent, not evidence of the previous caller or of Codex.
    fn host(&self, meta: Option<&Value>) -> Result<Host, String> {
        if let Some(meta) = meta.filter(|meta| meta.get(VERSION_KEY).is_some()) {
            return Ok(host_info(meta.get(CLIENT_INFO).unwrap_or(&Value::Null)));
        }
        self.host.get().cloned().ok_or_else(|| "the session did not introduce itself with `initialize`".to_string())
    }

    /// A modern caller with no explicit identity gets a request-scoped
    /// arrival. The transport is not evidence that two calls are one
    /// conversation (MCP 2026-07-28, Basic / Statelessness).
    pub fn for_call(self: &Arc<Self>, meta: Option<&Value>) -> Arc<Self> {
        let modern = meta.is_some_and(|meta| meta.get(VERSION_KEY).is_some());
        let identified = self.resident.is_some()
            || self.conversation.is_some()
            || meta.is_some_and(|meta| {
                ["threadId", "claudecode/toolUseId"]
                    .iter()
                    .any(|key| meta.get(key).and_then(Value::as_str).is_some_and(|id| !id.is_empty()))
            });
        if !modern || identified {
            return self.clone();
        }
        Arc::new(Self {
            world: self.world.clone(),
            host: OnceLock::new(),
            host_process: self.host_process.clone(),
            resident: self.resident.clone(),
            conversation: self.conversation.clone(),
            residents: tokio::sync::Mutex::new(HashMap::new()),
            were: Mutex::new(HashMap::new()),
            starting: tokio::sync::Mutex::new(()),
            lifetime_reported: Mutex::new(None),
            writes_off: self.writes_off,
            watching: Mutex::new(None),
            watched: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Whether the host is Codex, whose models may call the tools from an
    /// `exec` script and see only what it prints (`world-view.md`).
    pub fn codex(&self, meta: Option<&Value>) -> bool {
        self.host(meta).is_ok_and(|host| host.agent.as_str() == "codex")
    }

    /// Whether the World can start a turn here when something arrives for
    /// this session: in a Codex thread the World started, through its
    /// app-server, and in Claude Code, through the World's plugin. Any other
    /// session, such as Codex the user opened, learns it only with the
    /// answer of its next World tool call.
    pub fn woken(&self, meta: Option<&Value>) -> bool {
        self.resident.is_some() || self.host(meta).is_ok_and(|host| host.agent.as_str() == "claude")
    }

    pub async fn run(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let whereabouts = match self.whereabouts(meta).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return Answer::error(format!("=== run (not run: {error}) ===")),
        };
        let mut sent = false;
        let mut answer = self.run_answer(arguments, meta, &whereabouts, &mut sent).await;
        let call_id = meta
            .and_then(|meta| meta.get("claudecode/toolUseId").or_else(|| meta.get("callId")))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        if answer.is_error
            && !sent
            && let Some(id) = call_id
        {
            let recorded = async {
                let mut client = self.client(meta).await?;
                let error = answer
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        Content::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                client
                    .call(Call::RecordRunFailure { resident: whereabouts.resident.clone(), call_id: id.into(), error })
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            if let Err(error) = recorded {
                answer.follow(format!("recording tool call facts: {error}"));
            }
        }
        answer
    }

    async fn run_answer(
        self: &Arc<Self>,
        arguments: &Value,
        meta: Option<&Value>,
        whereabouts: &Whereabouts,
        sent: &mut bool,
    ) -> Answer {
        let command = match arguments.get("command") {
            Some(Value::String(command)) => command.clone(),
            // Some hosts drop or reshape what the model wrote (#34); saying
            // what arrived tells the model, and whoever reads the log, which.
            other => {
                return Answer::error(format!(
                    "=== run (not run: `command` must be a string holding the script; {}) ===",
                    arrived(arguments, other)
                ));
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
        let machine = fastctx_hands::run::preread::on_only(&command).ok().map(|(machine, _)| machine);
        let reply = async {
            let mut client = self.client(meta).await.map_err(crate::run_failure::Failure::before_dispatch)?;
            let call = Call::Run {
                call_id: meta.and_then(|meta| {
                    meta.get("claudecode/toolUseId")
                        .or_else(|| meta.get("callId"))
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                }),
                resident: whereabouts.resident.clone(),
                request: RunRequest {
                    turn: None,
                    call_id: None,
                    script: command,
                    cwd,
                    writes_off: self.writes_off,
                    wait,
                    attachments: Vec::new(),
                },
                node: None,
            };
            // The node records failures after accepting Run; the one-use host
            // proof must never be consumed again to record the same call.
            *sent = true;
            client.call(call).await.map_err(crate::run_failure::Failure::from)
        }
        .await;
        let answering = match &reply {
            Ok(Reply::Run(reply)) => Some(reply.job.clone()),
            _ => None,
        };
        let mut answer = match reply {
            Ok(Reply::Run(reply)) => {
                let (content, regions) = fastctx_hands::run::present::projected(&reply);
                let call_id = meta
                    .and_then(|meta| meta.get("claudecode/toolUseId").or_else(|| meta.get("callId")))
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty());
                let facts = call_id.zip(reply.run.clone()).map(|(id, facts)| crate::answer::FactView {
                    resident: whereabouts.resident.clone(),
                    call_id: id.to_string(),
                    facts,
                    regions,
                    full_output: fastctx_types::run_facts::OutputRef {
                        policy: reply
                            .run
                            .as_ref()
                            .and_then(|run| run.output.as_ref())
                            .and_then(|output| output.policy.clone()),
                        content: reply
                            .run
                            .as_ref()
                            .and_then(|run| run.output.as_ref())
                            .and_then(|output| output.content.clone()),
                        machine: reply.machine.clone(),
                        path: reply.output.log.clone(),
                        bytes: reply.output.bytes,
                        lines: reply.output.lines,
                        cut: false,
                    },
                });
                Answer {
                    facts,
                    content,
                    is_error: false,
                    receipt: None,
                    source: Some(format!(
                        "{} {}",
                        reply.machine,
                        fastctx_hands::run::present::display_path(&reply.output.log)
                    )),
                    shown: shown_whole(&reply).then(|| reply.job.clone()),
                    completed: (!matches!(
                        reply.state,
                        fastctx_types::run::RunState::Running | fastctx_types::run::RunState::Exited { .. }
                    ))
                    .then(|| fastctx_types::run::CompletedJob {
                        machine: reply.machine.clone(),
                        job: reply.job.clone(),
                        output: reply.output.log.clone(),
                    }),
                }
            }
            Ok(other) => Answer::error(format!("=== run (the node answered something else: {other:?}) ===")),
            Err(error) => error.answer(machine.as_deref()),
        };
        self.add_waiting_words_about(whereabouts, answering, &mut answer, meta).await;
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
            Some(Value::String(node)) => match fastctx_types::access::Written::parse(node) {
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
            let mut client = self.client(meta).await?;
            let call = Call::Write { job: None, resident: Some(whereabouts.resident.clone()), op, node, town: None };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Written { outcome: Ok(result) }) => Answer::text(result.text, false),
            Ok(Reply::Written { outcome: Err(error) }) => Answer::error(error.to_string()),
            Ok(other) => Answer::error(format!("=== {tool} (the node answered something else: {other:?}) ===")),
            Err(error) => Answer::error(format!("=== {tool} (not done: {error}) ===")),
        };
        self.add_waiting_words(&whereabouts, &mut answer, meta).await;
        answer
    }

    /// `ask_colleague`: a colleague for this session's resident.
    pub async fn ask(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let failed = |error: String| Answer::error(format!("=== ask_colleague (no colleague asked for: {error}) ==="));
        let (prompt, label) = match (text_argument(arguments, "prompt"), text_argument(arguments, "description")) {
            (Ok(prompt), Ok(label)) => (prompt, label),
            (Err(error), _) | (_, Err(error)) => return failed(error),
        };
        // A session listed before colleagues lived where the user says may
        // still name a machine.
        if arguments.get("node").is_some_and(|node| !node.is_null()) {
            return failed(
                "ask_colleague has no `node`: callees use the logins configured on the Model collaboration page; say in `prompt` where the colleague works".to_string(),
            );
        }
        let whereabouts = match self.whereabouts(meta).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return failed(error),
        };
        let reply = async {
            let mut client = self.client(meta).await?;
            let callee = match arguments.get("callee").filter(|value| !value.is_null()) {
                Some(_) => Some(text_argument(arguments, "callee")?),
                None => None,
            };
            let login = self.login(&self.host(meta)?);
            let call = Call::Ask { resident: whereabouts.resident.clone(), label, prompt, callee, login };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Asked { colleague }) => Answer {
                facts: None,
                content: vec![Content::Text(fastctx_types::collab_prompt::started(
                    &colleague.resident,
                    (whereabouts.machine.as_ref() != Some(&colleague.machine)).then_some(&colleague.machine),
                ))],
                is_error: false,
                receipt: None,
                source: None,
                shown: None,
                completed: None,
            },
            Ok(other) => Answer::error(format!("=== ask_colleague (the node answered something else: {other:?}) ===")),
            Err(error) => failed(error),
        };
        self.add_waiting_words(&whereabouts, &mut answer, meta).await;
        answer
    }

    /// `message_user`: words of this session's resident for the user.
    pub async fn say(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let failed = |error: String| Answer::error(format!("=== message_user (not sent: {error}) ==="));
        let message = match text_argument(arguments, "message") {
            Ok(message) => message,
            Err(error) => return failed(error),
        };
        let status = match arguments.get("status").and_then(Value::as_str) {
            Some("reply") => fastctx_types::residents::TellStatus::Reply,
            Some("info") => fastctx_types::residents::TellStatus::Info,
            Some("urgent") => fastctx_types::residents::TellStatus::Urgent,
            _ => return failed("status is one of reply, info, urgent".to_string()),
        };
        let attachments: Vec<fastctx_types::voice::Attachment> = match arguments.get("attachments") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => match serde_json::from_value(value.clone()) {
                Ok(attachments) => attachments,
                Err(_) => return failed("each attachment names its machine and its path".to_string()),
            },
        };
        if attachments.len() > fastctx_types::voice::ATTACHMENTS_MOST {
            return failed(format!("at most {} attachments", fastctx_types::voice::ATTACHMENTS_MOST));
        }
        let whereabouts = match self.whereabouts(meta).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return failed(error),
        };
        let reply = async {
            let mut client = self.client(meta).await?;
            let call = Call::Say { resident: whereabouts.resident.clone(), message, status, attachments };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Told) => Answer {
                facts: None,
                content: vec![Content::Text(fastctx_types::voice::TOLD.to_string())],
                is_error: false,
                receipt: None,
                source: None,
                shown: None,
                completed: None,
            },
            Ok(other) => Answer::error(format!("=== message_user (the node answered something else: {other:?}) ===")),
            Err(error) => failed(error),
        };
        self.add_waiting_words(&whereabouts, &mut answer, meta).await;
        answer
    }

    /// `message_colleague`: words from this session's resident to another.
    pub async fn message(self: &Arc<Self>, arguments: &Value, meta: Option<&Value>) -> Answer {
        let failed = |error: String| Answer::error(format!("=== message_colleague (not sent: {error}) ==="));
        let (to, message) = match (text_argument(arguments, "to"), text_argument(arguments, "message")) {
            (Ok(to), Ok(message)) => (to, message),
            (Err(error), _) | (_, Err(error)) => return failed(error),
        };
        let (to, machine) = match to.split_once('@') {
            Some((to, machine)) => match (ResidentId::try_from(to), fastctx_types::access::Written::parse(machine)) {
                (Ok(to), Ok(machine)) => (to, Some(machine)),
                (Err(error), _) | (_, Err(error)) => return failed(error.to_string()),
            },
            None => match ResidentId::try_from(to.as_str()) {
                Ok(id) => (id, None),
                Err(error) => return failed(error.to_string()),
            },
        };
        let whereabouts = match self.whereabouts(meta).await {
            Ok(whereabouts) => whereabouts,
            Err(error) => return failed(error),
        };
        let reply = async {
            let mut client = self.client(meta).await?;
            let call = Call::Message {
                resident: whereabouts.resident.clone(),
                to,
                machine,
                text: message,
                login: self.login(&self.host(meta)?),
            };
            client.call(call).await.map_err(|error| error.to_string())
        }
        .await;
        let mut answer = match reply {
            Ok(Reply::Spoken { to, landed, .. }) => {
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
                    facts: None,
                    content: vec![Content::Text(format!(
                        "=== message to {} on {} ({how}) ===\nThe last message of the turn it goes into will be delivered to you.",
                        to.resident, to.machine
                    ))],
                    is_error: false,
                    receipt: None,
                    source: None,
                    shown: None,
                    completed: None,
                }
            }
            Ok(other) => {
                Answer::error(format!("=== message_colleague (the node answered something else: {other:?}) ==="))
            }
            Err(error) => failed(error),
        };
        self.add_waiting_words(&whereabouts, &mut answer, meta).await;
        answer
    }

    /// What the resident is told of where it lives, and words waiting for
    /// it, ride along with its answer, each in a block of its own
    /// (`world-colleagues.md`, 话怎样送到居民眼前; `world-projects.md`).
    async fn add_waiting_words(&self, whereabouts: &Whereabouts, answer: &mut Answer, meta: Option<&Value>) {
        self.add_waiting_words_about(whereabouts, None, answer, meta).await;
    }

    /// What waits for the resident, added to an answer that itself tells of
    /// the job `answering`.
    async fn add_waiting_words_about(
        &self,
        whereabouts: &Whereabouts,
        answering: Option<fastctx_types::JobId>,
        answer: &mut Answer,
        meta: Option<&Value>,
    ) {
        let collect =
            Call::Collect { resident: whereabouts.resident.clone(), answering, subagent: whereabouts.subagent };
        let mut client = match self.client(meta).await {
            Ok(client) => client,
            Err(error) => {
                answer.follow(format!("=== words for you could not be read: {error} ==="));
                return;
            }
        };
        let collected = client.call(collect).await.map_err(|error| error.to_string());
        match collected {
            Ok(Reply::Notices { notices, notes, leading_notes }) => {
                let here = whereabouts.machine.as_ref();
                let messages = notices.into_iter().map(|delivery| (delivery.id, told::told(&delivery.notice, here)));
                let ids = match answer::assemble(&self.world, answer, notes, leading_notes, messages) {
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
        let host = self.host(meta)?;
        let tool_use_id = meta.and_then(|meta| meta.get("claudecode/toolUseId"));
        if host.agent.as_str() == "claude" || tool_use_id.is_some() {
            let tool_use_id = tool_use_id.and_then(Value::as_str).filter(|id| !id.is_empty())
                .ok_or("Claude did not supply a nonempty _meta[\"claudecode/toolUseId\"]; this call cannot be paired with its mod session")?;
            let hints = Hints {
                resident: self.known_resident(meta),
                tool_use_id: Some(tool_use_id.to_string()),
                ..Hints::default()
            };
            let mut client = self.client(meta).await?;
            return match client.call(Call::Arrive { host, hints }).await {
                Ok(Reply::Arrived(whereabouts)) => {
                    self.were.lock().expect("never poisoned").insert(None, whereabouts.resident.clone());
                    Ok(whereabouts)
                }
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
            match known.client.call(Call::Machines { resident: None, login: None, thread: None, scratch: false }).await
            {
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
            .map(|address| address.resident)
            .or_else(|| self.conversation.clone());
        let home = self.login(&host).and_then(|login| login.home);
        let hints = Hints {
            host_process: self.host_process.clone(),
            resident,
            thread: thread.clone(),
            was,
            tool_use_id: None,
            home,
        };
        let mut client = self.client_for(host.clone()).await?;
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

    /// The machines this session works on, for its tools' descriptions, and
    /// whether it may ask for colleagues. Listing the tools starts the node
    /// when nothing keeps it, as on a computer in no network: a host that
    /// lists them once must see them as they will be. None while a service
    /// keeps the node and it is not running.
    pub async fn machines(&self, meta: Option<&Value>) -> Result<Option<crate::tools::Machines>, String> {
        let host = self.host(meta)?;
        let mut client = match self.connecting(host.clone()).await {
            Ok(client) => client,
            Err(fastctx_client::ConnectError::KeptByService { .. }) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        match client
            .call(Call::Machines {
                resident: self.known_resident(meta),
                login: self.login(&host),
                thread,
                scratch: true,
            })
            .await
        {
            Ok(Reply::Machines { here, others, groups, collab, mut scratch, speaks }) => {
                if !client.scratch_locations {
                    scratch = here
                        .iter()
                        .cloned()
                        .chain(others.iter().cloned())
                        .map(|name| (name, Err(fastctx_types::scratch::words::OLD_NODE.into())))
                        .collect();
                }
                Ok(Some(crate::tools::Machines { here, others, groups, collab, scratch, speaks }))
            }
            Ok(other) => Err(format!("the node answered something else: {other:?}")),
            Err(error) => Err(error.to_string()),
        }
    }

    fn login(&self, host: &Host) -> Option<fastctx_types::collab::Caller> {
        let key = if host.agent.as_str() == "codex" { "CODEX_HOME" } else { "CLAUDE_CONFIG_DIR" };
        let explicit = std::env::var_os("FASTCTX_AGENT_HOME");
        let inherited = std::env::var_os(key);
        if explicit.is_none() && inherited.is_none() && std::env::var_os("FASTCTX_LOGIN_IDENTITY").is_none() {
            return None;
        }
        let home = explicit.or(inherited).filter(|home| !home.is_empty()).map(std::path::PathBuf::from);
        Some(fastctx_types::collab::Caller { agent: host.agent.clone(), home })
    }

    /// This session's resident, when it is known without asking the node:
    /// the World started it, or one of its calls arrived.
    pub fn known_resident(&self, meta: Option<&Value>) -> Option<ResidentId> {
        if let Some(address) = self.resident.as_deref().and_then(|address| address.parse::<Address>().ok()) {
            return Some(address.resident);
        }
        if let Some(conversation) = &self.conversation {
            return Some(conversation.clone());
        }
        let thread = meta.and_then(|meta| meta.get("threadId")).and_then(Value::as_str).map(str::to_string);
        self.were.lock().expect("never poisoned").get(&thread).cloned()
    }

    /// Tells the host each time the tools this session lists may have
    /// changed, for as long as the node it reached stays up; started again
    /// with each call that reaches a node, so a restarted node is watched
    /// too, and told of once.
    pub fn watch_tools(self: &Arc<Self>, output: &crate::stdio::Output, meta: Option<&Value>) {
        let resident = self.known_resident(meta);
        let Ok(host) = self.host(meta) else { return };
        {
            let mut watching = self.watching.lock().expect("never poisoned");
            if watching.as_ref().is_some_and(|(watched, task)| *watched == resident && !task.is_finished()) {
                return;
            }
            if let Some((_, task)) = watching.take() {
                task.abort();
            }
            let (world, output, first) =
                (self.world.clone(), output.clone(), !self.watched.swap(true, std::sync::atomic::Ordering::AcqRel));
            let watched = resident.clone();
            let task = tokio::spawn(async move {
                let Ok(client) = Client::connect(&world, Peer::Session { host }).await else { return };
                let Ok(mut changes) = client.watch_tools(Call::WatchTools { resident: watched }).await else { return };
                // What changed while no node was watched is told now.
                if !first {
                    output.send(&crate::tools::list_changed());
                }
                while changes.next().await.is_ok() {
                    output.send(&crate::tools::list_changed());
                }
            });
            *watching = Some((resident, task));
        }
    }

    pub fn stop_watching(&self) {
        if let Some((_, task)) = self.watching.lock().expect("never poisoned").take() {
            task.abort();
        }
    }

    pub(crate) async fn client(&self, meta: Option<&Value>) -> Result<Client, String> {
        self.client_for(self.host(meta)?).await
    }

    async fn client_for(&self, host: Host) -> Result<Client, String> {
        self.connecting(host).await.map_err(|error| error.to_string())
    }

    /// A connection to the node, started first when nothing keeps it; its
    /// containment is told on standard error once for each node process.
    async fn connecting(&self, host: Host) -> Result<Client, fastctx_client::ConnectError> {
        let _starting = self.starting.lock().await;
        let client = Client::connect_or_start(&self.world, Peer::Session { host }).await?;
        if let Some(lifetime) = &client.node_lifetime {
            let mut reported = self.lifetime_reported.lock().expect("never poisoned");
            if reported.as_ref() != Some(&lifetime.process) {
                if let Some(warning) = lifetime.warning() {
                    eprintln!("fastctx: {warning}");
                }
                *reported = Some(lifetime.process.clone());
            }
        }
        Ok(client)
    }
}

fn host_info(client_info: &Value) -> Host {
    let name = client_info.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    let version = client_info.get("version").and_then(Value::as_str).unwrap_or_default().to_string();
    Host { agent: agent_of(&name), client: name, version }
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
            encoding: None,
            file_path: text("file_path")?.into(),
            old_string: text("old_string")?,
            new_string: text("new_string")?,
            replace_all: match arguments.get("replace_all") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(all)) => *all,
                Some(_) => return Err("`replace_all` must be true or false".to_string()),
            },
        }),
        "write" => WriteOp::Write(WriteRequest {
            file_path: text("file_path")?.into(),
            content: text("content")?,
            encoding: None,
        }),
        "apply_patch" => WriteOp::ApplyPatch(ApplyPatchRequest { input: text("input")? }),
        other => return Err(format!("{other} is not a writing tool")),
    })
}

/// What a call's arguments held where `command` was expected.
fn arrived(arguments: &Value, command: Option<&Value>) -> String {
    let kind = |value: &Value| match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    };
    match (arguments, command) {
        (Value::Object(_), Some(command)) => format!("it arrived as {}", kind(command)),
        (Value::Object(fields), None) if fields.is_empty() => "the arguments arrived empty".to_string(),
        (Value::Object(fields), None) => {
            let names: Vec<String> = fields.keys().map(|name| format!("`{name}`")).collect();
            format!("the arguments held only {}", names.join(", "))
        }
        (other, _) => format!("the arguments arrived as {}, not an object", kind(other)),
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

/// A string argument.
fn text_argument(arguments: &Value, name: &str) -> Result<String, String> {
    match arguments.get(name) {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(format!("`{name}` must be a string")),
        None => Err(format!("`{name}` is missing")),
    }
}

/// Whether a run's answer shows all its job did: it ended within the call
/// and its whole output is in the answer.
fn shown_whole(reply: &fastctx_types::run::RunReply) -> bool {
    use fastctx_types::run::{OutputPiece, RunState};
    matches!(reply.state, RunState::Ended { .. })
        && reply.output.pieces.iter().all(|piece| matches!(piece, OutputPiece::Text { .. } | OutputPiece::Image { .. }))
}
