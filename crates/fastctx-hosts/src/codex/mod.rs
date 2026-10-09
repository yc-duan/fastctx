//! Codex residents: threads of the one long-lived `codex app-server` this
//! machine's node holds (`world-colleagues.md`, 世界怎样拉起 GPT 同事).
//!
//! The host turns the app-server's protocol into what the World does with a
//! session: start a thread, get it ready for words, send it words, stop its
//! turn, read its history. What the app-server reports about a thread comes
//! out as session events (`fastctx_types::session`); what Codex's
//! notifications mean is known here and nowhere else.
//!
//! Presentation follows the official 0.160 app-server contract, including
//! experimental state needed by the desktop clients. Raw reasoning and
//! explicitly hidden UI items do not become visible session activities.
//! `environments: []` disables the thread's own shell and file editing.

mod agents;
mod answers;
#[cfg(test)]
mod contract;
mod controls;
mod history;
mod items;
pub mod launch;
mod lifetime;
mod management;
mod notifications;
mod offers;
mod presentation;
mod server;
mod transport;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use fastctx_types::session::{
    Happened, Item, Part, SessionChange, SessionEvent, Step, StepStatus, TurnEnd, TurnHow, What,
};
use jiff::Timestamp;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};

use launch::Launch;
use lifetime::ServerUse;
use server::{AppServer, Inbound, Unanswered};

pub type Log = Arc<dyn Fn(String) + Send + Sync>;
pub type EventBarrier = Arc<Mutex<Option<oneshot::Sender<Result<(), String>>>>>;

/// How long Codex waits for a thread's tool server to start; the World
/// gives Codex this, and waits as long for word of how it went.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// How long Codex waits for one call of a World tool: a minute above the
/// longest wait a `run` may ask for (`fastctx_hands::run::LONGEST_WAIT`),
/// for the answer that follows the wait.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(660);

/// The MCP/history limit for an answer of at most `budget` units across its
/// displayed text blocks, each block costing its UTF-8 bytes divided by four,
/// rounded up, plus images. The caller must include its own head notes and
/// delivered messages in that budget. For Code Mode, it bounds the text the
/// script prints, not the raw MCP JSON it can inspect.
///
/// Verified against desktop Codex 0.160.0, upstream commit
/// a956835d020762cb2b570053af06f643a11c0ecc:
/// - `protocol/src/models.rs` preserves text/image blocks as content items;
///   the World sends neither `structuredContent` nor unknown block types.
/// - `core/src/tools/context.rs` prepends a separate text item:
///   `Wall time: {duration.as_secs_f64():.4} seconds\nOutput:`. It has no tool
///   name, call id, exit code or trailing newline. Even `Duration::MAX`, rounded
///   to 2^64 seconds by f64, has only 20 integer digits: the header is at most
///   11 + (20 + 1 + 4) + 16 = 52 ASCII bytes. The 300-second MCP timeout is not
///   a bound on the handler's elapsed time, which includes hooks and approvals.
/// - `utils/output-truncation/src/lib.rs` charges each text item separately at
///   ceil(UTF-8 bytes / 4), without separators: the units the World counts
///   its budget in. Images consume none of this budget. The header costs at
///   most 13 tokens.
/// - Code Mode skips that MCP header/truncation and returns raw MCP JSON to
///   the script. Its exec/wait output has a different header, subsequently
///   checked against the root limit in history (`core/src/tools/code_mode/`).
///   The runtime's cell id is a decimal u64, at most 20 digits. A restarted
///   host can prepend `g{generation}:`, another 22 characters, making
///   `Script running with cell ID {id}` at most 28 + 42 = 70 ASCII bytes.
///   Host time crosses both pipe and gRPC transports as u64 nanoseconds
///   (`code-mode-protocol/src/host/payload.rs`, `code-mode/src/grpc_session/`).
///   The usual `.1`-second header therefore has at most 70 + 28 + 13 = 111
///   bytes. With experimental_show_cell_overhead, it is replaced by:
///   `{status}\nWall time {total:.3} seconds (code-mode {host:.3} seconds; overhead {difference:.3} seconds)\nOutput:\n`.
///   Total time is a u64 millisecond count, host time a u64 nanosecond count;
///   the widest numbers are 21, 15 and 21 characters respectively (negative
///   overhead needs at most 16). With 68 fixed bytes, this is at most
///   70 + 68 + 21 + 15 + 21 = 195 bytes, or ceil(195 / 4) = 49 tokens.
///   A handler duration overflowing u64 milliseconds keeps the shorter header.
///   These are alternative paths: the 13-token MCP header is not added again.
/// - `protocol/src/protocol.rs` applies the allowance as
///   `(limit as f64 * 1.2).ceil()`. Set limit = ceil(5 * (budget + 49) / 6),
///   rounding upwards rather than discarding a fraction. For 80,000 units
///   this is 66,708, whose allowance is 80,050 tokens, covering 80,049.
///   A byte-mode root policy gets four bytes per configured token before the
///   allowance: ceil(66,708 * 4 * 1.2) = 320,199 covers 320,000 + 195 bytes.
///   Direct MCP history reuses its already computed allowance from metadata;
///   Code Mode history uses the root policy. Neither adds another header.
///
/// Code Mode also truncates prints *before* adding its header, under each exec's
/// `max_output_tokens` pragma or wait's `max_tokens` argument (each defaults to
/// 10,000, without the 20% allowance; wait does not inherit the exec budget).
/// That limit must independently accommodate the prints; changing this setting
/// cannot prevent earlier truncation, arbitrary script expansion, or hook edits.
pub fn output_limit(budget: usize) -> u64 {
    const HEADER_TOKENS: u128 = 195_u128.div_ceil(4);
    // Widen before adding and multiplying, including on 64-bit targets.
    ((budget as u128 + HEADER_TOKENS) * 5).div_ceil(6) as u64
}

/// The name of the World's tool server in a thread; the startup it reports
/// under this name decides whether the thread has the World's hands.
pub const SERVER_NAME: &str = "fastctx";

/// What a thread is started and resumed with. Kept, because `thread/resume`
/// does not remember it, and a thread is resumed after every restart of the
/// app-server.
#[derive(Clone, Debug)]
pub struct ThreadSpec {
    pub cwd: PathBuf,
    pub developer_instructions: String,
    /// Settings for this thread alone, laid over the user's configuration as
    /// a `-c` would be. Tables merge key by key and anything else replaces.
    pub config: Value,
    /// What the thread is called where Codex lists threads, such as the
    /// desktop app's sidebar; given once, when it is started. Without one,
    /// the client that shows it first names it from its words.
    pub name: Option<String>,
}

/// Words for a thread.
#[derive(Clone, Debug)]
pub enum Input {
    /// What the model reads as the next message to it.
    Text(String),
    /// Explicitly invokes the skill selected from this thread's catalog.
    Skill { name: String, path: String, text: String },
    /// Separate text blocks retain the attachment envelopes in host history.
    Messages(Vec<InputMessage>),
    /// A fact shaped as a tool's output, such as a job's end.
    ToolOutput { name: String, namespace: Option<String>, output: String },
}

#[derive(Clone, Debug)]
pub struct InputMessage {
    pub text: String,
    pub images: Vec<String>,
}

/// Where words went: a turn that started with them, or the running turn,
/// which they join at its next tool boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    pub turn: String,
    pub joined: bool,
}

/// Why words sent to a thread are not known to be in a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// They did not go in.
    NotSent(String),
    /// The app-server stopped as it took them: whether they went in is not
    /// known, and if they did, the turn they went into ended with it.
    Unknown(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotSent(why) | SendError::Unknown(why) => f.write_str(why),
        }
    }
}

/// What the host tells the node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum HostEvent {
    /// A read barrier for the consumer's durable event queue. Never persisted.
    #[serde(skip)]
    Synchronized {
        thread: String,
        done: EventBarrier,
    },
    Unloaded {
        thread: String,
    },
    Happened {
        thread: String,
        event: SessionEvent,
    },
    /// A thread used this machine's Computer Use: a call started, or one
    /// finished, refused or not by the app's helper.
    Desk {
        thread: String,
        turn: String,
        desk: Desk,
    },
    /// The app-server stopped, and these turns, as (thread, turn), with it;
    /// `how` is how it exited, such as `exit code: 1`. Threads come back
    /// with the next words.
    Gone {
        how: String,
        turns: Vec<(String, String)>,
    },
}

/// A use of the desktop app's Computer Use, which works one request at a
/// time and refuses the others without saying who holds it
/// (`world-colleagues.md`, 一台机器只有一张桌子).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Desk {
    Calling,
    Done,
    /// The helper refused the call: another request held it.
    Refused,
}

/// The tool server through which a thread uses the app's Windows Computer
/// Use.
const COMPUTER_USE: &str = "node_repl";
/// What the app's helper answers while another request is in flight.
const HELPER_BUSY: &str = "Computer Use helper already has an active request";

pub struct CodexHost {
    codex_home: PathBuf,
    workdir: PathBuf,
    env: Vec<(OsString, OsString)>,
    log: Log,
    events: mpsc::UnboundedSender<HostEvent>,
    state: Mutex<State>,
    // A notification and replacement of its server are one transition.
    generation: Mutex<()>,
    /// Starting the app-server is done by one caller at a time.
    starting: tokio::sync::Mutex<()>,
    /// Resuming is done one thread at a time: a second resume of a loaded
    /// thread starts none of its tool servers again, so nothing would say
    /// how they started.
    loading: tokio::sync::Mutex<()>,
    /// Woken whenever the app-server says how a thread's World tools
    /// started.
    tools_changed: Notify,
}

#[derive(Default)]
struct State {
    notification_sequence: u64,
    server_users: usize,
    server: Option<(Arc<AppServer>, Launch)>,
    threads: HashMap<String, Thread>,
}

#[derive(Default)]
struct Thread {
    selection: controls::Selection,
    model: Option<String>,
    effort: Option<String>,
    usage: Option<Value>,
    metadata: Value,
    requests: HashMap<String, answers::Request>,
    created_here: bool,
    completed_item: bool,
    spec: Option<ThreadSpec>,
    loaded: bool,
    subscribed: bool,
    /// The turn running now: reported started, or named in the answer to
    /// the words that started it.
    turn: Option<String>,
    submitted: bool,
    observed_submission: Option<String>,
    /// Turns reported started and not yet ended. A turn the app-server took
    /// but could not set up never starts, and the error it reports is its
    /// only end.
    started: HashSet<String>,
    /// Turns that ended: an answer to `turn/start` that arrives after its
    /// turn ended must not make it run again.
    finished: HashSet<String>,
    /// Ends reported by the host but not yet made durable by the node.
    unsettled: HashSet<String>,
    /// Errors for turns not yet named by the answer that started them.
    failed_early: HashMap<String, String>,
    /// The agent's last message so far, and the turn it is in.
    last_message: Option<(String, String)>,
    /// The app-server said how the thread's World tools started since it was
    /// last started or resumed.
    tools_settled: bool,
    tools_revision: u64,
    /// Why the World's tools are not there in the thread's current load.
    tools_missing: Option<String>,
    items: HashMap<(String, String), Value>,
    item_starts: HashMap<(String, String), Timestamp>,
}

impl CodexHost {
    /// The configuration directory used by this host and its status commands.
    pub fn config_home(&self) -> &Path {
        &self.codex_home
    }

    /// A host for the Codex configured in `codex_home`; its app-server runs
    /// in `workdir` with exactly the environment `env`, and starts at the
    /// first call that needs it.
    pub fn new(
        codex_home: PathBuf,
        workdir: PathBuf,
        env: Vec<(OsString, OsString)>,
        log: Log,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<HostEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let host = Arc::new(Self {
            codex_home,
            workdir,
            env,
            log,
            events,
            state: Mutex::new(State::default()),
            generation: Mutex::new(()),
            starting: tokio::sync::Mutex::new(()),
            loading: tokio::sync::Mutex::new(()),
            tools_changed: Notify::new(),
        });
        (host, receiver)
    }

    /// Makes a thread started before known again, as after the node
    /// restarted; it is resumed when words next go to it.
    pub fn adopt(&self, thread: &str, spec: ThreadSpec) {
        self.lock().threads.entry(thread.to_string()).or_default().spec = Some(spec);
    }

    /// Starts a thread without a turn, so the caller can record it before
    /// any event of its first turn. Persists the empty thread before returning
    /// its ID and model, so an idle app-server replacement can resume it.
    pub async fn start_thread(self: &Arc<Self>, spec: ThreadSpec) -> Result<(String, Option<String>), String> {
        let server = self.server().await?;
        let answer = server
            .request(
                "thread/start",
                json!({
                    "cwd": spec.cwd,
                    "approvalPolicy": "never",
                    "sandbox": "danger-full-access",
                    "developerInstructions": spec.developer_instructions,
                    "config": spec.config,
                    "ephemeral": false,
                    "historyMode": "paginated",
                    "environments": [],
                }),
            )
            .await?;
        let thread = answer
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or("thread/start answered without a thread id")?
            .to_string();
        let model = answer.get("model").and_then(Value::as_str).map(str::to_string);
        server.persist_new_thread(&thread).await?;
        // Codex keeps a name given before the thread's first turn and writes
        // it with the thread (0.160). A thread without the name still works.
        if let Some(name) = &spec.name
            && let Err(error) = server.request("thread/name/set", json!({ "threadId": thread, "name": name })).await
        {
            (self.log)(format!("naming the Codex thread {thread} {name:?} failed: {error}"));
        }
        // What the app-server said of the thread, how its tools started
        // among it, may have come before this answer.
        let mut state = self.lock();
        let current = matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server));
        if !current {
            return Err("the Codex app-server stopped while creating the thread".into());
        }
        let entry = state.threads.entry(thread.clone()).or_default();
        entry.spec = Some(spec);
        entry.loaded = current;
        entry.subscribed = current;
        entry.created_here = true;
        entry.model = model.clone();
        entry.effort = answer.get("reasoningEffort").and_then(Value::as_str).map(str::to_string);
        entry.metadata = answer["thread"].clone();
        Ok((thread, model))
    }

    /// How the user's own Codex joins `thread` in this host's app-server:
    /// the program the app-server runs, the address of its socket, and the
    /// Codex directory it reads. The thread is loaded first, with the World's
    /// settings, so the program joins it rather than loading it with its own.
    pub async fn attachable(self: &Arc<Self>, thread: &str) -> Result<(PathBuf, String, PathBuf), String> {
        self.prepare(thread).await?;
        let program = self
            .lock()
            .server
            .as_ref()
            .map(|(_, launch)| launch.binary.clone())
            .ok_or("the Codex app-server stopped before it could be joined")?;
        let socket = std::path::absolute(&self.workdir)
            .map_err(|error| format!("finding {}: {error}", self.workdir.display()))?
            .join("app-server.sock");
        Ok((program, format!("unix://{}", socket.display()), self.codex_home.clone()))
    }

    /// Makes a thread ready for words: loaded in the app-server, and its
    /// World tools started or given up on, since Codex fixes a turn's tools
    /// when the turn starts and does not wait for a server still starting.
    /// Returns why the World's tools are missing, when they are.
    pub async fn prepare(self: &Arc<Self>, thread: &str) -> Result<Option<String>, String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        let settled = async {
            loop {
                let notified = self.tools_changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.lock().threads.get(thread).is_none_or(|entry| entry.tools_settled) {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(STARTUP_TIMEOUT, settled).await.is_err() {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            entry.tools_settled = true;
            entry.tools_missing = Some(format!(
                "Codex said nothing about starting the World's tools within {}s; whether they are there is not known",
                STARTUP_TIMEOUT.as_secs()
            ));
        }
        Ok(self.lock().threads.get(thread).and_then(|entry| entry.tools_missing.clone()))
    }

    /// Sends words to a thread: they start a turn when it is idle and join
    /// the running turn when it is busy, which the answer tells by the turn
    /// it names. A thread not loaded is resumed first; call
    /// [`Self::prepare`] before to have its tools there.
    pub async fn send(self: &Arc<Self>, thread: &str, input: Input) -> Result<Sent, SendError> {
        let server = self.server().await.map_err(SendError::NotSent)?;
        self.ensure_loaded(&server, thread).await.map_err(SendError::NotSent)?;
        let mut params = json!({
            "threadId": thread,
            "input": [],
            // A remote client may have selected a local environment since
            // our last turn. Restore the World's empty default for this turn.
            "environments": [],
        });
        match input {
            Input::Text(text) => params["input"] = json!([{ "type": "text", "text": text }]),
            Input::Skill { name, path, text } => {
                params["input"] = json!([
                    {"type": "text", "text": text},
                    {"type": "skill", "name": name, "path": path}
                ]);
            }
            Input::Messages(messages) => {
                let mut input = Vec::new();
                for message in messages {
                    input.push(json!({ "type": "text", "text": message.text }));
                    for path in message.images {
                        input.push(json!({ "type": "localImage", "path": path }));
                    }
                }
                params["input"] = json!(input);
            }
            Input::ToolOutput { name, namespace, output } => {
                params["toolOutput"] = json!({ "name": name, "namespace": namespace, "output": output })
            }
        }
        // Whether they join a running turn is what ran before they went in:
        // the turn they start may be reported started before the answer.
        let before = {
            let mut state = self.lock();
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
                return Err(SendError::NotSent("the app-server changed before submission".to_string()));
            }
            let entry = state.threads.entry(thread.to_string()).or_default();
            // Turn settings also affect a steered turn in 0.160. A pending
            // choice belongs only to the next turn the World starts.
            if entry.turn.is_none() {
                entry.selection.apply(&mut params);
            }
            entry.submitted = true;
            entry.observed_submission = None;
            entry.turn.clone()
        };
        let response = server.ask("turn/start", params).await;
        let observed = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            entry.submitted = false;
            entry.observed_submission.take()
        };
        let answer = match response {
            Ok(answer) => answer,
            Err(Unanswered::Refused(why)) => return Err(SendError::NotSent(why)),
            Err(Unanswered::Rpc { message, .. }) => return Err(SendError::NotSent(message)),
            Err(Unanswered::Stopped(why)) => {
                if let Some(turn) = observed {
                    return Ok(Sent { joined: before.as_deref() == Some(turn.as_str()), turn });
                }
                return Err(SendError::Unknown(why));
            }
        };
        let turn = answer
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| SendError::Unknown("turn/start answered without a turn id".to_string()))?
            .to_string();
        let failed = {
            let mut state = self.lock();
            // An app-server that stopped, or was replaced, while it answered
            // runs nothing more: the turn they went into ended with it.
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
                return Ok(Sent { joined: before.as_deref() == Some(turn.as_str()), turn });
            }
            let entry = state.threads.entry(thread.to_string()).or_default();
            if entry.turn.as_deref() != Some(turn.as_str()) && !entry.finished.contains(&turn) {
                entry.turn = Some(turn.clone());
                entry.last_message = None;
            }
            entry.failed_early.remove(&turn)
        };
        if let Some(reason) = failed {
            self.end(thread, &turn, TurnHow::Failed, Some(reason), None, Timestamp::now());
        }
        let joined = before.as_deref() == Some(turn.as_str());
        Ok(Sent { turn, joined })
    }

    /// Stops a thread's running turn; `false` when none runs. The turn's end
    /// comes as usual, from the app-server.
    pub async fn interrupt(self: &Arc<Self>, thread: &str) -> Result<bool, String> {
        let Some(turn) = self.running_turn(thread) else {
            return Ok(false);
        };
        let server = self.server().await?;
        server.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn })).await?;
        Ok(true)
    }

    /// The thread's history so far, as session events. A turn still running
    /// is not ended in it, whatever the record says.
    pub async fn history(self: &Arc<Self>, thread: &str) -> Result<Vec<SessionEvent>, String> {
        let server = self.server().await?;
        // A new thread is durable, but has no completed items to read yet.
        if self.lock().threads.get(thread).is_some_and(|t| t.created_here && !t.completed_item) {
            return Ok(Vec::new());
        }
        let running = self.running_turn(thread);
        history::read(&server, thread, running.as_deref()).await
    }

    /// Wait until the node has applied every event sent before this barrier.
    pub async fn flush_events(&self, thread: &str) -> Result<(), String> {
        let (done, applied) = oneshot::channel();
        self.events
            .send(HostEvent::Synchronized { thread: thread.into(), done: Arc::new(Mutex::new(Some(done))) })
            .map_err(|_| "the Codex event consumer stopped".to_string())?;
        applied.await.map_err(|_| "the Codex event consumer stopped before applying its queue".to_string())?
    }

    /// Whether a turn runs in any thread this host holds.
    pub fn any_turn_running(&self) -> bool {
        self.lock().threads.values().any(|entry| entry.turn.is_some())
    }

    /// No turn runs and every reported end has been retained or applied by
    /// the node. A host becoming idle alone does not make handoff safe.
    pub fn settled(&self) -> bool {
        self.lock().threads.values().all(|entry| entry.turn.is_none() && entry.unsettled.is_empty())
    }

    /// A report's effects are durable, or its unclaimed contents are kept
    /// for the next node. Only then can its turn cease preventing handoff.
    pub fn acknowledge(&self, event: &HostEvent) {
        let mut state = self.lock();
        let mut settled = |thread: &str, turn: &str| {
            if let Some(entry) = state.threads.get_mut(thread) {
                entry.unsettled.remove(turn);
            }
        };
        match event {
            HostEvent::Happened { thread, event: SessionEvent { happened: Happened::TurnEnded { turn, .. }, .. } } => {
                settled(thread, turn);
            }
            HostEvent::Gone { turns, .. } => {
                for (thread, turn) in turns {
                    settled(thread, turn);
                }
            }
            _ => {}
        }
    }

    pub fn running_turn(&self, thread: &str) -> Option<String> {
        self.lock().threads.get(thread).and_then(|entry| entry.turn.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("never poisoned")
    }

    async fn ensure_loaded(&self, server: &Arc<AppServer>, thread: &str) -> Result<(), String> {
        let _loading = self.loading.lock().await;
        let spec = {
            let mut state = self.lock();
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, server)) {
                return Err("the app-server changed before loading the thread".to_string());
            }
            let Some(entry) = state.threads.get_mut(thread) else {
                return Err(format!("no Codex thread {thread} is known on this machine"));
            };
            if entry.loaded && entry.subscribed {
                return Ok(());
            }
            // Resuming starts the thread's tool servers again; what they say
            // from here on counts.
            entry.tools_settled = false;
            entry.tools_missing = None;
            entry.spec.clone().ok_or_else(|| format!("how the Codex thread {thread} was started is not known"))?
        };
        let selection = controls::Selection::read(&self.workdir, thread)?;
        let answer = server
            .request(
                "thread/resume",
                json!({
                    "threadId": thread,
                    "cwd": spec.cwd,
                    "approvalPolicy": "never",
                    "sandbox": "danger-full-access",
                    "developerInstructions": spec.developer_instructions,
                    "config": spec.config,
                    "excludeTurns": true,
                }),
            )
            .await?;
        let mut state = self.lock();
        if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, server)) {
            return Err("the app-server stopped while loading the thread".to_string());
        }
        let entry = state.threads.entry(thread.to_string()).or_default();
        // Another client may have started a turn while resume's reply was
        // in flight. Preserve any turn already reported by a notification.
        entry.loaded = true;
        entry.subscribed = true;
        entry.selection = selection;
        entry.model = answer.get("model").and_then(Value::as_str).map(str::to_string);
        entry.effort = answer.get("reasoningEffort").and_then(Value::as_str).map(str::to_string);
        entry.metadata = answer["thread"].clone();
        Ok(())
    }

    /// The running app-server, started on first use or after it stopped.
    /// A replacement waits for running turns and callers using the current
    /// process. Its identity includes the app's binary and Computer Use
    /// connection, not incidental writes to the user's configuration.
    async fn server(self: &Arc<Self>) -> Result<ServerUse, String> {
        let _starting = self.starting.lock().await;
        {
            let mut state = self.lock();
            let busy = state.server_users != 0
                || state.threads.values().any(|thread| thread.turn.is_some() || thread.submitted);
            if busy && let Some((server, _)) = &state.server {
                let server = server.clone();
                state.server_users += 1;
                return Ok(ServerUse::new(server, self.clone()));
            }
        }
        let path = self.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("PATH")).map(|(_, value)| value);
        let launch = launch::resolve(&self.codex_home, path)?;
        let replaced = {
            let _generation = self.generation.lock().expect("never poisoned");
            let mut state = self.lock();
            let busy = state.server_users != 0
                || state.threads.values().any(|thread| thread.turn.is_some() || thread.submitted);
            match &state.server {
                Some((server, previous)) if previous.same_runtime(&launch) || busy => {
                    let server = server.clone();
                    state.server_users += 1;
                    return Ok(ServerUse::new(server, self.clone()));
                }
                Some((server, _)) => {
                    (self.log)(
                        "the Codex program or desktop tool connection changed; restarting the app-server".to_string(),
                    );
                    server.kill();
                    let replaced = server.clone();
                    self.resolve_requests(
                        &mut state,
                        "the app-server was replaced; its requests are no longer answerable",
                    );
                    state.server = None;
                    Some(replaced)
                }
                None => None,
            }
        };
        if let Some(replaced) = replaced {
            replaced.wait().await;
        }
        let (server, inbound) =
            AppServer::spawn(&launch.binary, &launch.args, &self.workdir, &self.env, self.log.clone())
                .map_err(|error| format!("starting {}: {error}", launch.binary.display()))?;
        {
            let _generation = self.generation.lock().expect("never poisoned");
            let mut state = self.lock();
            for (id, thread) in &mut state.threads {
                if thread.loaded {
                    let _ = self.events.send(HostEvent::Unloaded { thread: id.clone() });
                }
                thread.loaded = false;
                thread.subscribed = false;
                thread.turn = None;
                thread.requests.clear();
                thread.usage = None;
            }
            state.server = Some((server.clone(), launch.clone()));
        }
        tokio::spawn(pump(server.clone(), inbound, Arc::downgrade(self)));
        let started = server
            .request(
                "initialize",
                json!({
                    "clientInfo": { "name": "fastctx", "title": "FastCtx World", "version": env!("CARGO_PKG_VERSION") },
                    // `environments` is an experimental field.
                    "capabilities": { "experimentalApi": true },
                }),
            )
            .await;
        let started = started.and_then(|answer| server.notify("initialized", json!({})).map(|()| answer));
        let answer = match started {
            Ok(answer) => answer,
            Err(error) => {
                server.kill();
                return Err(format!("starting the Codex app-server {}: {error}", launch.binary.display()));
            }
        };
        (self.log)(format!(
            "Codex app-server {} started as pid {} from {}",
            answer.get("userAgent").and_then(Value::as_str).unwrap_or("(no user agent)"),
            server.pid(),
            launch.binary.display()
        ));
        let mut state = self.lock();
        if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
            return Err("the app-server exited during initialization".to_string());
        }
        state.server_users += 1;
        Ok(ServerUse::new(server, self.clone()))
    }

    /// Records that a turn ended and reports it with its last message.
    fn end(
        &self,
        thread: &str,
        turn: &str,
        how: TurnHow,
        reason: Option<String>,
        duration_ms: Option<u64>,
        at: Timestamp,
    ) {
        self.end_with(thread, turn, how, reason, duration_ms, at, None, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn end_with(
        &self,
        thread: &str,
        turn: &str,
        how: TurnHow,
        reason: Option<String>,
        duration_ms: Option<u64>,
        at: Timestamp,
        summary: Option<String>,
        error: Option<Value>,
    ) {
        let last_message = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            // One end a turn: a second report of it changes nothing.
            if !entry.finished.insert(turn.to_string()) {
                return;
            }
            entry.unsettled.insert(turn.to_string());
            entry.started.remove(turn);
            entry.items.retain(|(item_turn, _), _| item_turn != turn);
            entry.item_starts.retain(|(item_turn, _), _| item_turn != turn);
            if entry.turn.as_deref() == Some(turn) {
                entry.turn = None;
            }
            let said = match &entry.last_message {
                Some((said_in, _)) if said_in == turn => entry.last_message.take().map(|(_, text)| text),
                _ => None,
            };
            summary.or(said)
        };
        let end = TurnEnd { error, how, reason, duration_ms, last_message };
        self.emit(thread, at, Happened::TurnEnded { turn: turn.to_string(), end });
    }

    fn emit(&self, thread: &str, at: Timestamp, happened: Happened) {
        let event = SessionEvent { at, happened };
        let _ = self.events.send(HostEvent::Happened { thread: thread.to_string(), event });
    }

    /// Reads one notification about a thread.
    fn notified(&self, method: &str, params: &Value, at: Timestamp) {
        self.observe_controls(method, params);
        let revision = {
            let mut state = self.lock();
            state.notification_sequence += 1;
            Some(state.notification_sequence)
        };
        let Some(thread) =
            params.get("threadId").or_else(|| params.pointer("/thread/id")).and_then(Value::as_str).map(str::to_string)
        else {
            return;
        };
        let text = |pointer: &str| params.pointer(pointer).and_then(Value::as_str).map(str::to_string);
        let turn = text("/turnId").or_else(|| self.running_turn(&thread));
        if let Some(mut happened) = notifications::read(method, params) {
            if let Happened::Changed { change: SessionChange::Resolved { id, .. }, .. } = &mut happened {
                *id = self.request_address(id);
            }
            if method == "thread/realtime/item/completed" {
                self.lock().threads.entry(thread.clone()).or_default().completed_item = true;
            }
            if method == "thread/started"
                && let Some(title) =
                    params.pointer("/thread/name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
            {
                self.emit(&thread, at, Happened::Titled { title: title.into() });
            }
            if method == "guardianWarning" {
                if turn.is_none() {
                    return;
                }
                if let Happened::Changed { turn: warning_turn, .. } = &mut happened {
                    *warning_turn = turn.clone();
                }
            }
            self.emit(&thread, at, happened);
            return;
        }
        match method {
            "turn/started" => {
                let Some(turn) = text("/turn/id") else { return };
                {
                    let mut state = self.lock();
                    let entry = state.threads.entry(thread.clone()).or_default();
                    if entry.finished.contains(&turn) {
                        return;
                    }
                    if entry.submitted && entry.turn.as_deref() != Some(turn.as_str()) {
                        entry.observed_submission = Some(turn.clone());
                    }
                    if entry.turn.as_deref() != Some(turn.as_str()) {
                        entry.turn = Some(turn.clone());
                        entry.last_message = None;
                    }
                    entry.started.insert(turn.clone());
                }
                let at = seconds(params.pointer("/turn/startedAt"));
                self.emit(&thread, at, Happened::TurnStarted { turn });
            }
            "item/started" | "item/completed" => {
                let (Some(item), Some(turn)) = (params.get("item"), turn) else { return };
                let completed = method == "item/completed";
                let at = millis(params.get(if completed { "completedAtMs" } else { "startedAtMs" }));
                let computer = item.get("type").and_then(Value::as_str) == Some("mcpToolCall")
                    && item.get("server").and_then(Value::as_str) == Some(COMPUTER_USE);
                if computer {
                    let desk = match completed {
                        false => Desk::Calling,
                        true if item.to_string().contains(HELPER_BUSY) => Desk::Refused,
                        true => Desk::Done,
                    };
                    let _ = self.events.send(HostEvent::Desk { thread: thread.clone(), turn: turn.clone(), desk });
                }
                // Words that went in are told by whoever sent them, who knows
                // who they are from.
                {
                    let mut state = self.lock();
                    let entry = state.threads.entry(thread.clone()).or_default();
                    entry.completed_item |= completed;
                    let key = (turn.clone(), item.get("id").and_then(Value::as_str).unwrap_or_default().to_string());
                    if completed {
                        entry.items.remove(&key);
                    } else {
                        entry.items.insert(key, item.clone());
                    }
                }
                let mut item = match items::read(item, completed) {
                    items::Read::Words { text, message } => {
                        if completed {
                            self.emit(
                                &thread,
                                at,
                                Happened::Heard {
                                    delivery: None,
                                    turn: Some(turn),
                                    from: None,
                                    text,
                                    message: Some(Box::new(message)),
                                },
                            );
                        }
                        return;
                    }
                    items::Read::Item(item) => item,
                };
                let mut state = self.lock();
                let entry = state.threads.entry(thread.clone()).or_default();
                let key = (turn.clone(), item.id.clone());
                if completed {
                    item.started_at = entry.item_starts.remove(&key);
                } else {
                    item.started_at = Some(at);
                    entry.item_starts.insert(key, at);
                }
                drop(state);
                let happened = if completed {
                    if let What::Message { text } = &item.what {
                        self.lock().threads.entry(thread.clone()).or_default().last_message =
                            Some((turn.clone(), text.clone()));
                    }
                    Happened::Finished { turn, item }
                } else {
                    Happened::Began { turn, item }
                };
                self.emit(&thread, at, happened);
            }
            "item/agentMessage/delta" | "item/plan/delta" => {
                let (Some(turn), Some(item), Some(delta)) = (turn, text("/itemId"), text("/delta")) else { return };
                self.emit(&thread, at, Happened::Grew { revision, turn, item, part: Part::Text, text: delta });
            }
            "item/reasoning/summaryTextDelta" | "item/fileChange/patchUpdated" => {
                let (Some(turn), Some(id)) = (turn, text("/itemId")) else { return };
                let snapshot = {
                    let mut state = self.lock();
                    let entry = state.threads.entry(thread.clone()).or_default();
                    let item = entry.items.entry((turn.clone(), id.clone())).or_insert_with(|| {
                        if method == "item/fileChange/patchUpdated" {
                            json!({"id": id, "type": "fileChange", "status": "inProgress", "changes": []})
                        } else {
                            json!({"id": id, "type": "reasoning", "summary": []})
                        }
                    });
                    if method == "item/fileChange/patchUpdated" {
                        item["changes"] = params["changes"].clone();
                    } else {
                        let Some(index) =
                            params.get("summaryIndex").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok())
                        else {
                            return;
                        };
                        let Some(delta) = text("/delta") else { return };

                        let summary = item["summary"].as_array_mut().expect("reasoning summary array");
                        summary.resize(summary.len().max(index + 1), json!(""));
                        let part = summary[index].as_str().unwrap_or_default().to_string() + &delta;
                        summary[index] = json!(part);
                    }
                    item.clone()
                };
                if let items::Read::Item(mut item) = items::read(&snapshot, false) {
                    item.started_at = self
                        .lock()
                        .threads
                        .get(&thread)
                        .and_then(|entry| entry.item_starts.get(&(turn.clone(), item.id.clone())).copied());
                    self.emit(&thread, at, Happened::Updated { turn, item });
                }
            }
            "item/commandExecution/outputDelta" => {
                let (Some(turn), Some(item), Some(delta)) = (turn, text("/itemId"), text("/delta")) else { return };
                self.emit(&thread, at, Happened::Grew { revision, turn, item, part: Part::Output, text: delta });
            }
            "turn/plan/updated" => {
                let Some(turn) = turn else { return };
                let steps = params
                    .get("plan")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|step| Step {
                        id: None,
                        active_form: None,
                        detail: Some(step.clone()),
                        text: step.get("step").and_then(Value::as_str).unwrap_or_default().to_string(),
                        status: match step.get("status").and_then(Value::as_str) {
                            Some("completed") => StepStatus::Done,
                            Some("inProgress") => StepStatus::Doing,
                            _ => StepStatus::Pending,
                        },
                    })
                    .collect();
                self.emit(&thread, at, Happened::Steps { turn, steps, note: text("/explanation") });
            }
            "thread/tokenUsage/updated" => {
                let Some(used) = params.pointer("/tokenUsage/last/totalTokens").and_then(Value::as_u64) else { return };
                let window = params.pointer("/tokenUsage/modelContextWindow").and_then(Value::as_u64);
                self.emit(&thread, at, Happened::Context { used, window });
                self.emit(
                    &thread,
                    at,
                    Happened::Changed { turn, change: SessionChange::Usage { usage: params["tokenUsage"].clone() } },
                );
            }
            "thread/name/updated" => {
                let Some(title) = text("/threadName") else { return };
                let title = title.trim().to_string();
                if title.is_empty() {
                    return;
                }
                self.emit(&thread, at, Happened::Titled { title });
            }
            "turn/completed" => {
                let Some(id) = text("/turn/id") else { return };
                let how = turn_how(text("/turn/status").as_deref());
                let reason = text("/turn/error/message");
                let duration = params.pointer("/turn/durationMs").and_then(Value::as_u64);
                let at = seconds(params.pointer("/turn/completedAt"));
                // The completion names the turn's last message itself, when
                // it has one; it is the turn's own, whatever came before.
                let summary = params.pointer("/turn/items").and_then(Value::as_array).and_then(|items| {
                    items.iter().rev().find_map(|item| match items::read(item, true) {
                        items::Read::Item(Item { what: What::Message { text }, .. }) => Some(text),
                        _ => None,
                    })
                });
                self.end_with(
                    &thread,
                    &id,
                    how,
                    reason,
                    duration,
                    at,
                    summary,
                    params.pointer("/turn/error").filter(|e| !e.is_null()).cloned(),
                );
            }
            "error" => {
                self.emit(
                    &thread,
                    at,
                    Happened::Changed {
                        turn,
                        change: SessionChange::Error {
                            error: params["error"].clone(),
                            will_retry: params.get("willRetry").and_then(Value::as_bool).unwrap_or(false),
                        },
                    },
                );
                self.error(&thread, params, at);
            }
            "mcpServer/startupStatus/updated" => {
                let (name, status) = (text("/name").unwrap_or_default(), text("/status").unwrap_or_default());
                let why = text("/error").unwrap_or_else(|| "no reason given".to_string());
                if name == SERVER_NAME {
                    // A thread the World starts has no hands but these. Their
                    // start comes before any words, so it is told with the
                    // words that find them missing.
                    let missing = match status.as_str() {
                        "failed" => Some(format!("the World's tools did not start: {why}")),
                        "cancelled" => Some("Codex cancelled starting the World's tools".to_string()),
                        _ => None,
                    };
                    {
                        let mut state = self.lock();
                        let entry = state.threads.entry(thread.clone()).or_default();
                        entry.tools_revision += 1;
                        entry.tools_settled = matches!(status.as_str(), "ready" | "failed" | "cancelled");
                        entry.tools_missing = missing;
                    }
                    self.tools_changed.notify_waiters();
                }
                self.emit(
                    &thread,
                    at,
                    Happened::Changed {
                        turn,
                        change: SessionChange::ToolServer {
                            server: name,
                            status,
                            error: params["error"].clone(),
                            failure_reason: params["failureReason"].clone(),
                        },
                    },
                );
            }

            "thread/closed" => {
                // Unloaded: the next words resume it.
                if let Some(entry) = self.lock().threads.get_mut(&thread) {
                    entry.loaded = false;
                    entry.subscribed = false;
                }
                let _ = self.events.send(HostEvent::Unloaded { thread });
            }
            _ => {}
        }
    }

    /// An error notification. With `willRetry` it is a reconnect and the turn
    /// goes on. Otherwise, an error in a started turn is followed by the
    /// turn's end, which carries it; for a turn that was taken but never
    /// started it is that turn's only end, and may come before the answer to
    /// `turn/start` that names the turn.
    fn error(&self, thread: &str, params: &Value, at: Timestamp) {
        let message = params.pointer("/error/message").and_then(Value::as_str).unwrap_or_default().to_string();
        let turn = params.get("turnId").and_then(Value::as_str).unwrap_or_default().to_string();
        if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let (awaited, started) = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            let awaited = entry.turn.as_deref() == Some(turn.as_str());
            let started = entry.started.contains(&turn);
            if !awaited && !started && !turn.is_empty() && !entry.finished.contains(&turn) {
                if entry.submitted {
                    entry.observed_submission = Some(turn.clone());
                    drop(state);
                    self.end(thread, &turn, TurnHow::Failed, Some(message), None, at);
                } else {
                    entry.failed_early.insert(turn, message);
                }
                return;
            }
            (awaited, started)
        };
        match (awaited, started) {
            (true, false) => self.end(thread, &turn, TurnHow::Failed, Some(message), None, at),
            (true, true) => {}
            (false, _) => {}
        }
    }

    /// The app-server stopped: if it is the current one, the turns it ran
    /// end with it, and every thread is resumed with its next words.
    fn exited(&self, server: &Arc<AppServer>, how: String) {
        let _generation = self.generation.lock().expect("never poisoned");
        let turns = {
            let mut state = self.lock();
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, server)) {
                // One replaced on purpose exits after its successor may have
                // started turns; those are not its own.
                (self.log)(format!("a replaced Codex app-server stopped ({how})"));
                return;
            }
            self.resolve_requests(&mut state, "the app-server stopped; its requests are no longer answerable");
            state.server = None;
            let mut turns = Vec::new();
            for (thread, entry) in &mut state.threads {
                entry.loaded = false;
                entry.requests.clear();
                if let Some(turn) = entry.turn.take() {
                    entry.started.remove(&turn);
                    entry.finished.insert(turn.clone());
                    entry.unsettled.insert(turn.clone());
                    entry.last_message = None;
                    turns.push((thread.clone(), turn));
                }
            }
            turns
        };
        (self.log)(format!("the Codex app-server stopped ({how})"));
        let _ = self.events.send(HostEvent::Gone { how, turns });
        for thread in self.lock().threads.keys() {
            let _ = self.events.send(HostEvent::Unloaded { thread: thread.clone() });
        }
    }
}

/// Reads what the app-server says, until it stops.
async fn pump(server: Arc<AppServer>, mut inbound: mpsc::UnboundedReceiver<Inbound>, host: Weak<CodexHost>) {
    while let Some(message) = inbound.recv().await {
        let Some(host) = host.upgrade() else { return };
        match message {
            Inbound::Response { waiter, answer } => {
                let _ = waiter.send(answer);
            }
            Inbound::Notification { method, params, emitted_at_ms } => {
                let _generation = host.generation.lock().expect("never poisoned");
                let current = matches!(&host.lock().server, Some((current, _)) if Arc::ptr_eq(current, &server));
                if current {
                    host.notified(
                        &method,
                        &params,
                        emitted_at_ms.and_then(|n| Timestamp::from_millisecond(n).ok()).unwrap_or_else(Timestamp::now),
                    );
                }
            }
            Inbound::Request { id, method, params } => {
                if matches!(
                    method.as_str(),
                    "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
                        | "item/tool/requestUserInput"
                        | "mcpServer/elicitation/request"
                        | "item/permissions/requestApproval"
                        | "applyPatchApproval"
                        | "execCommandApproval"
                ) {
                    if let Some(thread) =
                        params.get("threadId").or_else(|| params.get("conversationId")).and_then(Value::as_str)
                    {
                        let key = server.request_key(&id);
                        let at = Timestamp::now();
                        {
                            let _generation = host.generation.lock().expect("never poisoned");
                            let mut state = host.lock();
                            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
                                continue;
                            }
                            state.threads.entry(thread.into()).or_default().requests.insert(
                                key.to_string(),
                                answers::Request { at, id: id.clone(), method: method.clone(), params: params.clone() },
                            );
                        }
                        host.emit(
                            thread,
                            at,
                            Happened::Changed {
                                turn: params.get("turnId").and_then(Value::as_str).map(str::to_string),
                                change: SessionChange::Request {
                                    id: key,
                                    request: method.clone(),
                                    params: params.clone(),
                                },
                            },
                        );
                    }
                    // Requests are broadcast to subscribers; even an error
                    // would consume the user's opportunity to answer.
                    continue;
                }
                (host.log)(format!("declined the app-server's {method} request"));
                let _ = server.refuse(id, -32601, "The World does not answer this request.");
            }
            Inbound::Exited { why, drained } => {
                host.exited(&server, why);
                let _ = drained.send(());
                return;
            }
        }
    }
}

fn turn_how(status: Option<&str>) -> TurnHow {
    match status {
        Some("completed") => TurnHow::Completed,
        Some("failed") => TurnHow::Failed,
        Some("interrupted") => TurnHow::Interrupted,
        Some(other) => TurnHow::Other(other.to_string()),
        None => TurnHow::Other("unknown".to_string()),
    }
}

/// A Unix time in seconds from the protocol, or now when it gave none.
fn seconds(value: Option<&Value>) -> Timestamp {
    value
        .and_then(Value::as_i64)
        .and_then(|seconds| Timestamp::from_second(seconds).ok())
        .unwrap_or_else(Timestamp::now)
}

fn millis(value: Option<&Value>) -> Timestamp {
    value
        .and_then(Value::as_i64)
        .and_then(|millis| Timestamp::from_millisecond(millis).ok())
        .unwrap_or_else(Timestamp::now)
}

/// The settings that give a thread the World's tools, `tools`, in place of
/// any fastctx the user registered: `binary` serving them with the
/// environment `env`, and the limits above. Codex's own shell and image
/// viewer are off, as `run` does their work; its file editing goes with
/// the thread's `environments: []`. The tools are never deferred: a model
/// whose tools are deferred sees only their names, and `run` takes the place
/// of a shell whose description it always saw; in Code Mode they stay
/// callable from its scripts. A thread's settings
/// merge into the user's table by table, so every setting of the server
/// that changes what it offers is given here, lists replacing the user's
/// whole.
pub fn world_tools(binary: &Path, env: &[(String, String)], tools: &[&str], budget: usize) -> Value {
    let env: serde_json::Map<String, Value> =
        env.iter().map(|(name, value)| (name.clone(), Value::String(value.clone()))).collect();
    let limit = output_limit(budget);
    let limits: serde_json::Map<String, Value> =
        tools.iter().map(|name| (name.to_string(), json!({ "output_token_limit": limit }))).collect();
    json!({
        // Media is part of the watched session, including while it streams.
        "features": {
            "omit_app_server_notification_media": false,
            "shell_tool": false,
            "view_image": false,
        },
        "tool_output_token_limit": limit,
        "mcp_servers": { SERVER_NAME: {
            "command": binary,
            "args": ["serve"],
            "env": env,
            "env_vars": [],
            "enabled": true,
            "enabled_tools": tools,
            "disabled_tools": [],
            "omit_tools_from": ["deferred"],
            "startup_timeout_sec": STARTUP_TIMEOUT.as_secs(),
            "tool_timeout_sec": TOOL_TIMEOUT.as_secs(),
            "tools": limits,
        }},
    })
}
