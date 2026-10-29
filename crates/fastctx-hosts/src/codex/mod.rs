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

mod account;
pub mod assisted;
pub use account::{LimitsHow, LimitsReport, buckets, limits_of, merge};
mod accounting;
mod requests;
pub use accounting::recorded_turn_facts;
mod agents;
mod answers;
mod cache;
mod children;
mod commands;
mod compaction_history;
#[cfg(test)]
mod contract;
pub mod control_error;
mod control_start;
mod controls;
mod history;
pub mod identify;
mod image_position;
mod items;
pub mod launch;
mod lifetime;
mod management;
mod notifications;
mod offers;
mod presentation;
mod server;
mod steering;
pub mod submitted;
mod title;
mod tool_images;
mod transport;
mod user_config;
mod user_images;
mod user_picture;
pub use user_picture::UserPicture;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use fastctx_types::session::{
    Happened, Health, HealthKind, Item, Part, SessionChange, SessionEvent, Step, StepStatus, TurnEnd, TurnHow, What,
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
/// A runtime the app-server really starts reports its tools starting within
/// this; one it kept running says nothing of them, as they did not restart.
const RESTART_REPORTED_WITHIN: Duration = Duration::from_secs(10);
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
///   rounding upwards rather than discarding a fraction. For 50,000 units
///   this is 41,708, whose allowance is 50,050 tokens, covering 50,049.
///   A byte-mode root policy gets four bytes per configured token before the
///   allowance: ceil(41,708 * 4 * 1.2) = 200,199 covers 200,000 + 195 bytes.
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ThreadSpec {
    /// Resume a native thread with its own recorded settings, adding only MCP identity.
    #[serde(default)]
    pub native_resume: bool,
    #[serde(default)]
    pub user_mcp: std::collections::BTreeSet<String>,
    /// The resident whose user pictures this thread can address.
    pub owner: fastctx_types::Address,
    pub cwd: PathBuf,
    pub developer_instructions: String,
    /// Settings for this thread alone, laid over the user's configuration as
    /// a `-c` would be. Tables merge key by key and anything else replaces.
    pub config: Value,
    /// What the thread is called where Codex lists threads, such as the
    /// desktop app's sidebar; given once, when it is started. Without one,
    /// the client that shows it first names it from its words.
    pub name: Option<String>,
    /// Where the model and reasoning chosen for its next turns are kept,
    /// among its conversation's own records.
    pub choices: PathBuf,
}

impl ThreadSpec {
    /// A setting it was started with, such as its model, where it gives one.
    pub(super) fn started(&self, key: &str) -> Option<String> {
        self.config[key].as_str().map(str::to_string)
    }
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

/// How every client id the World gives its messages begins. The user's
/// words go in as typed, so this is how the World tells its own messages
/// from words typed into Codex itself.
pub const WORLD_CLIENT: &str = "fastctx:";

/// Whether a message in a thread is one the World sent.
pub fn sent_by_world(message: &fastctx_types::session::UserMessage) -> bool {
    message.client_id.as_deref().is_some_and(|id| id.starts_with(WORLD_CLIENT))
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

/// A thread Codex spawned as an agent of another thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawned {
    /// The thread whose agent it is.
    pub parent: String,
    pub nickname: Option<String>,
    pub role: Option<String>,
    pub model: Option<String>,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        facts: Vec<(String, String, fastctx_types::session::TurnFacts)>,
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
    assisted: Option<assisted::Assisted>,
    /// The home whose desktop app's program this host runs, when its own
    /// records none (`launch::discover_for`).
    program_home: Option<PathBuf>,
    workdir: PathBuf,
    env: Vec<(OsString, OsString)>,
    log: Log,
    events: mpsc::UnboundedSender<HostEvent>,
    state: Mutex<State>,
    finished: cache::Finished,
    // A notification and replacement of its server are one transition.
    generation: Mutex<()>,
    /// Starting the app-server is done by one caller at a time.
    starting: fastctx_platform::doing::Mutex<()>,
    /// Resuming is done one thread at a time: a second resume of a loaded
    /// thread starts none of its tool servers again, so nothing would say
    /// how they started.
    loading: fastctx_platform::doing::Mutex<()>,
    /// Woken whenever the app-server says how a thread's World tools
    /// started.
    tools_changed: Notify,
    /// Where what the app-server reads or pushes of the login's limits goes.
    limits: Mutex<Option<mpsc::UnboundedSender<account::LimitsReport>>>,
    limits_owner: Arc<Mutex<account::Owner>>,
    /// Where what each app-server says of the login on starting goes.
    said: Mutex<Option<mpsc::UnboundedSender<identify::SaidReport>>>,
}

struct State {
    notification_sequence: u64,
    server_users: usize,
    server: Option<(Arc<AppServer>, Launch)>,
    /// The running app-server's version, from its answer to `initialize`.
    version: Option<String>,
    threads: cache::Threads,
    /// The models the running app-server last listed. Choosing a model or an
    /// effort checks against it rather than asking again.
    catalog: Option<Vec<fastctx_types::session::control::ModelOffer>>,
    /// Hidden threads making a title now, each waiting for its turn to end
    /// (`title`). Only their attributed usage deltas reach the node.
    title_waiters: HashMap<String, oneshot::Sender<Result<String, String>>>,
    title_usage: HashMap<String, title::Usage>,
}

struct StateGuard<'a>(std::sync::MutexGuard<'a, State>);
impl std::ops::Deref for StateGuard<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        &self.0
    }
}
impl std::ops::DerefMut for StateGuard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.0
    }
}
impl Drop for StateGuard<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.0.threads.release();
        }
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Thread {
    #[serde(default)]
    submissions: user_images::Submissions,
    #[serde(default)]
    accounting: accounting::Accounting,
    #[serde(default)]
    request_timing: requests::Requests,
    selection: controls::Selection,
    model: Option<String>,
    effort: Option<String>,
    #[serde(default)]
    service_tier: Option<String>,
    usage: Option<Value>,
    metadata: Value,
    #[serde(skip)]
    requests: HashMap<String, answers::Request>,
    created_here: bool,
    completed_item: bool,
    spec: Option<ThreadSpec>,
    loaded: bool,
    #[serde(default)]
    configuration_changed: bool,
    subscribed: bool,
    #[serde(default)]
    children: HashSet<String>,
    #[serde(default)]
    host_active: bool,
    /// The turn running now: reported started, or named in the answer to
    /// the words that started it.
    turn: Option<String>,
    #[serde(skip)]
    start_revision: u64,
    submitted: bool,
    observed_submission: Option<String>,
    /// A stop asked for while words were on their way into a turn whose id
    /// was not known yet: that turn is stopped as soon as it is.
    #[serde(skip)]
    stop_wanted: bool,
    /// Turns reported started and not yet ended. A turn the app-server took
    /// but could not set up never starts, and the error it reports is its
    /// only end.
    started: HashSet<String>,
    /// Ends reported by the host but not yet made durable by the node.
    unsettled: HashSet<String>,
    /// The agent's last message so far, and the turn it is in.
    last_message: Option<(String, String)>,
    /// The app-server said how the thread's World tools started since it was
    /// last started or resumed.
    tools_settled: bool,
    tools_revision: u64,
    /// Why the World's tools are not there in the thread's current load.
    tools_missing: Option<String>,
    /// What was known of the World's tools when a resume was asked that may
    /// load the thread again, and the report count it was known at: what
    /// still holds when the app-server keeps the runtime it had.
    #[serde(skip)]
    tools_before: Option<(u64, Option<String>)>,
    #[serde(skip)]
    items: HashMap<(String, String), Value>,
    #[serde(skip)]
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
        program_home: Option<PathBuf>,
        workdir: PathBuf,
        env: Vec<(OsString, OsString)>,
        log: Log,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<HostEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let threads = cache::Threads::new(&workdir);
        let finished = cache::Finished::new(&workdir);
        let login = codex_home.display().to_string();
        let host = Arc::new(Self {
            codex_home,
            assisted: None,
            program_home,
            workdir,
            env,
            log,
            events,
            state: Mutex::new(State {
                threads,
                notification_sequence: 0,
                server_users: 0,
                server: None,
                version: None,
                catalog: None,
                title_waiters: HashMap::new(),
                title_usage: HashMap::new(),
            }),
            finished,
            generation: Mutex::new(()),
            starting: fastctx_platform::doing::Mutex::new(
                fastctx_platform::doing::Rank::CODEX_STARTING,
                "starting Codex",
                login.clone(),
                (),
            ),
            loading: fastctx_platform::doing::Mutex::new(
                fastctx_platform::doing::Rank::CODEX_LOADING,
                "loading a Codex thread",
                login,
                (),
            ),
            tools_changed: Notify::new(),
            limits: Mutex::new(None),
            limits_owner: Arc::new(Mutex::new(account::Owner::default())),
            said: Mutex::new(None),
        });
        (host, receiver)
    }

    /// Makes a thread started before known again, as after the node
    /// restarted; it is resumed when words next go to it.
    pub fn adopt(&self, thread: &str, spec: ThreadSpec) {
        let mut state = self.lock();
        let entry = state.threads.entry(thread.to_string()).or_default();
        entry.configuration_changed |= entry.spec.as_ref().is_some_and(|old| {
            old.native_resume != spec.native_resume || old.config != spec.config
                || old.cwd != spec.cwd
                || old.developer_instructions != spec.developer_instructions
        });
        entry.spec = Some(spec);
    }

    /// Starts a thread without a turn, so the caller can record it before
    /// any event of its first turn. Persists the empty thread before returning
    /// its ID and model, so an idle app-server replacement can resume it.
    pub async fn start_thread(self: &Arc<Self>, spec: ThreadSpec) -> Result<(String, Option<String>), String> {
        let server = self.server().await?;
        let effective = user_config::effective(&server, &spec, self.assisted.is_some()).await?;
        let answer = server
            .request(
                "thread/start",
                json!({
                    "cwd": spec.cwd,
                    "approvalPolicy": "never",
                    "sandbox": "danger-full-access",
                    "developerInstructions": effective.developer_instructions,
                    "config": effective.config,
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
        entry.accounting.empty_thread();
        entry.model = model.clone();
        entry.effort = answer.get("reasoningEffort").and_then(Value::as_str).map(str::to_string);
        entry.service_tier = answer.get("serviceTier").and_then(Value::as_str).map(str::to_string);
        entry.metadata = controls::metadata(&answer["thread"]);
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
        Ok((program, format!("unix://{}", socket.display()), self.session_home().to_path_buf()))
    }

    /// Makes a thread ready for words: loaded in the app-server, and its
    /// World tools started or given up on, since Codex fixes a turn's tools
    /// when the turn starts and does not wait for a server still starting.
    /// Returns why the World's tools are missing, when they are.
    pub async fn prepare(self: &Arc<Self>, thread: &str) -> Result<Option<String>, String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        // Resume may hand back the runtime the app-server already had, as it
        // does for one not idle: its tools did not restart, so no word of
        // them comes, and what was known of them still holds.
        let reported = async {
            loop {
                let notified = self.tools_changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.lock().threads.get(thread).is_none_or(|entry| {
                    entry.tools_settled || entry.tools_before.as_ref().is_none_or(|(at, _)| entry.tools_revision != *at)
                }) {
                    return;
                }
                notified.await;
            }
        };
        if tokio::time::timeout(RESTART_REPORTED_WITHIN, reported).await.is_err() {
            let mut state = self.lock();
            if let Some(entry) = state.threads.get_mut(thread)
                && !entry.tools_settled
                && let Some((at, missing)) = entry.tools_before.take()
                && entry.tools_revision == at
            {
                entry.tools_settled = true;
                entry.tools_missing = missing;
            }
        }
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
    /// [`Self::prepare`] before to have its tools there. `client` names the
    /// message Codex makes of them, in its live items and its history alike.
    pub async fn send(self: &Arc<Self>, thread: &str, input: Input, client: Option<String>) -> Result<Sent, SendError> {
        let (host, thread) = (self.clone(), thread.to_string());
        tokio::spawn(async move { host.send_inner(&thread, input, client).await })
            .await
            .map_err(|error| SendError::Unknown(format!("the turn submission task stopped: {error}")))?
    }

    async fn send_inner(
        self: &Arc<Self>,
        thread: &str,
        input: Input,
        client: Option<String>,
    ) -> Result<Sent, SendError> {
        let server = self.server().await.map_err(SendError::NotSent)?;
        self.ensure_loaded(&server, thread).await.map_err(SendError::NotSent)?;
        let mut params = json!({
            "threadId": thread,
            "input": [],
        });
        if !self.lock().threads.get(thread).and_then(|entry|entry.spec.as_ref()).is_some_and(|spec|spec.native_resume) {
            params["environments"] = json!([]);
        }
        if let Some(client) = client {
            params["clientUserMessageId"] = json!(client);
        }
        match input {
            Input::Text(text) => {
                params["input"] = json!([{ "type": "text", "text": text }]);
            }
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
        let (observed, stop) = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            entry.submitted = false;
            (entry.observed_submission.take(), std::mem::take(&mut entry.stop_wanted))
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
        {
            let mut state = self.lock();
            // An app-server that stopped, or was replaced, while it answered
            // runs nothing more: the turn they went into ended with it.
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
                return Ok(Sent { joined: before.as_deref() == Some(turn.as_str()), turn });
            }
            let entry = state.threads.entry(thread.to_string()).or_default();
            if entry.turn.as_deref() != Some(turn.as_str()) && !self.finished.contains(thread, &turn) {
                entry.turn = Some(turn.clone());
                entry.last_message = None;
            }
        }
        if stop && let Err(why) = server.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn })).await
        {
            let health = Health { kind: HealthKind::Error, text: format!("the turn could not be stopped: {why}") };
            self.emit(thread, Timestamp::now(), Happened::Health { turn: Some(turn.clone()), health });
        }
        let joined = before.as_deref() == Some(turn.as_str());
        Ok(Sent { turn, joined })
    }

    /// Stops a thread's running turn; `false` when none runs. The turn's end
    /// comes as usual, from the app-server. Words on their way start a turn
    /// whose id is not known yet: it is stopped once it is.
    pub async fn interrupt(self: &Arc<Self>, thread: &str) -> Result<bool, String> {
        let turn = {
            let mut state = self.lock();
            let Some(entry) = state.threads.get_mut(thread) else { return Ok(false) };
            match entry.turn.clone() {
                Some(turn) => turn,
                None if entry.submitted => {
                    entry.stop_wanted = true;
                    return Ok(true);
                }
                None => return Ok(false),
            }
        };
        let server = self.server().await?;
        server.request("turn/interrupt", json!({ "threadId": thread, "turnId": turn })).await?;
        Ok(true)
    }

    /// Interrupts only the observed turn; a later turn must never inherit this request.
    pub async fn interrupt_turn(self: &Arc<Self>, thread: &str, turn: &str) -> Result<bool, String> {
        if self.running_turn(thread).as_deref() != Some(turn) {
            return Ok(false);
        }
        self.server().await?.request("turn/interrupt", json!({"threadId":thread,"turnId":turn})).await?;
        Ok(true)
    }

    /// The thread's history so far, as session events, its user pictures
    /// addressed as `owner`'s. A turn still running is not ended in it,
    /// whatever the record says. The thread need not be one this host
    /// holds: Codex reads any thread of its home from its own record, such
    /// as one the user opened in the Codex command line.
    pub async fn history(
        self: &Arc<Self>,
        thread: &str,
        owner: &fastctx_types::Address,
    ) -> Result<Vec<SessionEvent>, String> {
        let server = self.server().await?;
        // A new thread is durable, but has no completed items to read yet.
        if self.lock().threads.get(thread).is_some_and(|t| t.created_here && !t.completed_item) {
            return Ok(Vec::new());
        }
        let running = self.running_turn(thread);
        match history::read(&server, thread, running.as_deref(), Some(owner)).await {
            // One a Codex of another version wrote reads once it is loaded.
            Err(error) if error == not_loaded(thread) => {
                self.ensure_loaded(&server, thread).await?;
                history::read(&server, thread, running.as_deref(), Some(owner)).await
            }
            read => read,
        }
    }

    /// What the thread is like now, as Codex says without reading its turns:
    /// it changes whenever the thread does, so equal stamps mean its history
    /// is as it was. A turn this host runs in it is part of it.
    pub async fn stamp(self: &Arc<Self>, thread: &str) -> Result<String, String> {
        let server = self.server().await?;
        let asking = json!({"threadId": thread, "includeTurns": false});
        let answer = match server.request("thread/read", asking.clone()).await {
            // One a Codex of another version wrote reads once it is loaded.
            Err(error) if error == not_loaded(thread) => {
                self.ensure_loaded(&server, thread).await?;
                server.request("thread/read", asking).await?
            }
            answer => answer?,
        };
        Ok(format!("{}|{:?}", answer["thread"], self.running_turn(thread)))
    }

    /// The rollout belongs to this host's thread. Only this host chooses
    /// its path; clients cannot supply a record belonging to another login.
    pub async fn rollout(self: &Arc<Self>, thread: &str) -> Result<Option<PathBuf>, String> {
        let answer =
            match self.server().await?.ask("thread/read", json!({"threadId":thread,"includeTurns":false})).await {
                Ok(answer) => answer,
                Err(Unanswered::Rpc { code: -32600, message, data })
                    if message == format!("thread not loaded: {thread}") && data.is_null() =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.to_string()),
            };
        Ok(answer["thread"]["path"].as_str().map(PathBuf::from))
    }

    /// Whose agent the thread is, where Codex spawned it for another thread
    /// (`multi_agent`): that thread, and the nickname and role Codex gave
    /// it. None for a thread no other spawned.
    pub async fn spawned_by(self: &Arc<Self>, thread: &str) -> Result<Option<Spawned>, String> {
        let answer =
            self.server().await?.request("thread/read", json!({"threadId": thread, "includeTurns": false})).await?;
        let found = &answer["thread"];
        let text =
            |key: &str| found.get(key).and_then(Value::as_str).map(str::to_string).filter(|text| !text.is_empty());
        Ok(text("parentThreadId").map(|parent| Spawned {
            parent,
            nickname: text("agentNickname"),
            role: text("agentRole"),
            model: text("model"),
        }))
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
    /// Whether this host started or adopted `thread`.
    /// The exact OS lifetime this host controls, when its server is running.
    pub fn process(&self) -> Option<fastctx_types::ProcessIdentity> {
        self.lock().server.as_ref().and_then(|(server,_)|server.process())
    }

    pub fn holds(&self, thread: &str) -> bool {
        self.lock().threads.get(thread).is_some_and(|entry|entry.spec.is_some())
    }

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

    /// A submission with no turn ID yet still owns the thread's next work boundary.
    pub fn busy(&self, thread: &str) -> bool {
        self.lock().threads.get(thread).is_some_and(|entry| entry.turn.is_some() || entry.submitted)
    }

    fn lock(&self) -> StateGuard<'_> {
        StateGuard(self.state.lock().expect("never poisoned"))
    }

    async fn ensure_loaded(&self, server: &Arc<AppServer>, thread: &str) -> Result<(), String> {
        let _loading = self.loading.lock().await;
        self.load_thread(server, thread).await?;
        self.resume_children(server, thread).await
    }

    async fn load_thread(&self, server: &Arc<AppServer>, thread: &str) -> Result<(), String> {
        let reload = self
            .lock()
            .threads
            .get(thread)
            .is_some_and(|entry| entry.configuration_changed && entry.loaded && entry.turn.is_none());
        if reload {
            let answer = server.request("thread/unsubscribe", json!({"threadId":thread})).await?;
            if answer["status"] == "notLoaded"
                && let Some(entry) = self.lock().threads.get_mut(thread)
            {
                entry.loaded = false;
                entry.subscribed = false;
            }
            // An idle, unsubscribed runtime is a cache entry. Resume with
            // changed overrides replaces it; waiting for its idle eviction
            // would otherwise take the host's full idle delay.
            if let Some(entry) = self.lock().threads.get_mut(thread) {
                entry.subscribed = false;
            }
        }
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
            // Subscribing to a still-loaded runtime does not restart its MCP
            // servers. Their previous startup receipt remains authoritative.
            // thread/closed is broadcast even to unsubscribed connections.
            if !entry.loaded || entry.configuration_changed {
                // A runtime known to have settled keeps what it had if it is
                // kept; one never seen settling is taken to have its tools.
                let known = if entry.tools_settled { entry.tools_missing.clone() } else { None };
                entry.tools_before = Some((entry.tools_revision, known));
                entry.tools_settled = false;
                entry.tools_missing = None;
                entry.accounting.reloaded();
            }
            entry.spec.clone().ok_or_else(|| format!("how the Codex thread {thread} was started is not known"))?
        };
        let selection = controls::Selection::read(&spec.choices)?;
        let params = if spec.native_resume {
            json!({"threadId":thread,"excludeTurns":true,"config":spec.config})
        } else {
            let effective = user_config::effective(server, &spec, self.assisted.is_some()).await?;
            json!({"threadId":thread,"cwd":spec.cwd,"approvalPolicy":"never","sandbox":"danger-full-access",
                "developerInstructions":effective.developer_instructions,"config":effective.config,"excludeTurns":true})
        };
        let answer = server.request("thread/resume",params).await?;
        let mut state = self.lock();
        if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, server)) {
            return Err("the app-server stopped while loading the thread".to_string());
        }
        let entry = state.threads.entry(thread.to_string()).or_default();
        // Another client may have started a turn while resume's reply was
        // in flight. Preserve any turn already reported by a notification.
        entry.configuration_changed = entry.spec.as_ref().is_some_and(|current| {
            current.native_resume != spec.native_resume || current.config != spec.config
                || current.developer_instructions != spec.developer_instructions
                || current.cwd != spec.cwd
        });
        if spec.native_resume && let Some(cwd) = answer.get("cwd").and_then(Value::as_str)
            && let Some(stored) = entry.spec.as_mut() {
            stored.cwd = PathBuf::from(cwd);
        }
        entry.loaded = true;
        entry.subscribed = true;
        entry.selection = selection;
        entry.model = answer.get("model").and_then(Value::as_str).map(str::to_string);
        entry.effort = answer.get("reasoningEffort").and_then(Value::as_str).map(str::to_string);
        entry.service_tier = answer.get("serviceTier").and_then(Value::as_str).map(str::to_string);
        entry.metadata = controls::metadata(&answer["thread"]);
        Ok(())
    }

    /// The running app-server, started on first use or after it stopped.
    /// A replacement waits for running turns and callers using the current
    /// process. Its identity includes the app's binary and Computer Use
    /// connection, not incidental writes to the user's configuration.
    fn server(
        self: &Arc<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ServerUse, String>> + Send + '_>> {
        Box::pin(self.server_inner())
    }

    async fn server_inner(self: &Arc<Self>) -> Result<ServerUse, String> {
        // Obtain the login authority before taking this host's startup lock;
        // the two hosts never wait for each other's startup locks.
        let authority = match &self.assisted {
            Some(assisted) => Some(assisted.owner.server().await?),
            None => None,
        };
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
        let mut launch = launch::resolve_for(&self.codex_home, self.program_home.as_deref(), path)?;
        if let (Some(assisted), Some(authority)) = (&self.assisted, &authority) {
            assisted.configure(authority, &mut launch).await?;
        }
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
                thread.host_active = false;
                thread.requests.clear();
                thread.usage = None;
            }
            state.server = Some((server.clone(), launch.clone()));
            state.version = None;
            state.catalog = None;
            state.threads.restarted();
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
        if let (Some(assisted), Some(authority)) = (&self.assisted, &authority)
            && let Err(error) = assisted.login(authority, &server).await
        {
            server.kill();
            return Err(error);
        }
        (self.log)(format!(
            "Codex app-server {} started as pid {} from {}",
            answer.get("userAgent").and_then(Value::as_str).unwrap_or("(no user agent)"),
            server.pid(),
            launch.binary.display()
        ));
        let version = identify::version_of(&answer);
        {
            let mut state = self.lock();
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
                return Err("the app-server exited during initialization".to_string());
            }
            state.version = version.clone();
            state.server_users += 1;
        }
        self.tell_said(server.clone(), launch, version);
        Ok(ServerUse::new(server, self.clone()))
    }

    /// Records that a turn ended and reports it with its last message.
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
        let (last_message, facts) = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            // One end a turn: a second report of it changes nothing.
            if !self.finished.insert(thread, turn) {
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
            (summary.or(said), entry.accounting.ended(turn))
        };
        let end = TurnEnd { facts: facts.map(Box::new), error, how, reason, duration_ms, last_message };
        self.emit(thread, at, Happened::TurnEnded { turn: turn.to_string(), end });
    }

    fn emit(&self, thread: &str, at: Timestamp, happened: Happened) {
        let event = SessionEvent { at, happened };
        let _ = self.events.send(HostEvent::Happened { thread: thread.to_string(), event });
    }

    /// Reads one notification about a thread.
    fn notified(&self, method: &str, params: &Value, at: Timestamp) {
        // The login's limits, which no thread owns.
        if method == "account/rateLimits/updated" {
            self.limits_heard(at, account::LimitsHow::Updated(params["rateLimits"].clone()));
            return;
        }
        // The login's skills changed on disk: every thread it holds offers
        // other commands from now on, and whoever looks asks again.
        if method == "skills/changed" {
            let threads: Vec<String> = {
                let mut state = self.lock();
                let loaded = state.threads.keys().cloned().collect::<Vec<_>>();
                loaded
                    .into_iter()
                    .filter(|thread| state.threads.get(thread).is_some_and(|entry| entry.loaded))
                    .collect()
            };
            for thread in threads {
                self.emit(&thread, at, Happened::Changed { turn: None, change: SessionChange::CommandsChanged });
            }
            return;
        }
        // A hidden title thread's notifications are its own: its turn's end
        // goes to whoever waits for it; usage alone goes to the root's ledger.
        if let Some(thread) =
            params.get("threadId").or_else(|| params.pointer("/thread/id")).and_then(Value::as_str).map(str::to_string)
        {
            let waiting = self.lock().title_waiters.contains_key(&thread);
            if waiting {
                self.title_usage(&thread, method, params, at);
                if method == "turn/completed" {
                    let title = title::read_turn_message(params).ok_or_else(|| {
                        format!(
                            "the title turn ended {}",
                            params.pointer("/turn/status").and_then(Value::as_str).unwrap_or("without a message")
                        )
                    });
                    if let Some(waiter) = self.lock().title_waiters.remove(&thread) {
                        let _ = waiter.send(title);
                    }
                }
                return;
            }
        }
        self.observe_controls(method, params);
        self.observe_children(method, params);
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
        let repeated_end = method == "turn/completed"
            && params
                .pointer("/turn/id")
                .and_then(Value::as_str)
                .is_some_and(|turn| self.finished.contains(&thread, turn));
        let failed =
            if repeated_end {
                None
            } else {
                let mut state = self.lock();
                let entry = state.threads.entry(thread.clone()).or_default();
                entry.request_timing.observe(method, params, at).map(|request| json!({ "counted": {
                "thread": thread, "turn": params.pointer("/turn/id").and_then(Value::as_str).or(turn.as_deref()),
                "model": entry.model, "effort": entry.effort, "tokens": null, "request": request,
            }}))
            };
        if let Some(usage) = failed {
            self.emit(&thread, at, Happened::Changed { turn: turn.clone(), change: SessionChange::Usage { usage } });
        }

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
                    if self.finished.contains(&thread, &turn) {
                        return;
                    }
                    if entry.submitted && entry.turn.as_deref() != Some(turn.as_str()) {
                        entry.observed_submission = Some(turn.clone());
                    }
                    if entry.turn.as_deref() != Some(turn.as_str()) {
                        entry.turn = Some(turn.clone());
                        entry.last_message = None;
                    }
                    entry.start_revision = entry.start_revision.wrapping_add(1);
                    entry.started.insert(turn.clone());
                    entry.submissions.started(&turn, entry.loaded && entry.subscribed);
                    entry.accounting.started(&turn);
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
                    if completed {
                        entry.submissions.observe(&turn, item);
                    }
                }
                // What it tells of Codex's own agents, as its record will.
                let facts = if completed { items::agent_facts(item) } else { Vec::new() };
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
                                    notice: None,
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
                    item.thought_until(at);
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
                    Happened::Finished { turn: turn.clone(), item }
                } else {
                    Happened::Began { turn: turn.clone(), item }
                };
                self.emit(&thread, at, happened);
                for change in facts {
                    self.emit(&thread, at, Happened::Changed { turn: Some(turn.clone()), change });
                }
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
                        // Only a reasoning item has a summary, of a few parts:
                        // what says otherwise is passed over, not followed.
                        let Some(summary) = item.get_mut("summary").and_then(Value::as_array_mut) else { return };
                        if index >= SUMMARY_PARTS {
                            return;
                        }
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
                let counted = {
                    let mut state = self.lock();
                    let entry = state.threads.entry(thread.clone()).or_default();
                    entry
                        .accounting
                        .usage(params.get("turnId").and_then(Value::as_str), &params["tokenUsage"])
                        .map(|tokens| json!({ "thread": thread, "turn": turn, "model": entry.model, "effort": entry.effort, "tokens": tokens,
                            "request": entry.request_timing.finish(at, fastctx_types::usage_calls::CallOutcome::Completed) }))
                };
                let mut usage = params["tokenUsage"].clone();
                if let Some(counted) = counted {
                    usage["counted"] = counted;
                }
                let Some(used) = params.pointer("/tokenUsage/last/totalTokens").and_then(Value::as_u64) else { return };
                let window = params.pointer("/tokenUsage/modelContextWindow").and_then(Value::as_u64);
                self.emit(&thread, at, Happened::Context { used, window });
                self.emit(&thread, at, Happened::Changed { turn, change: SessionChange::Usage { usage } });
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
                        entry.tools_before = None;
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
                    entry.host_active = false;
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
            if !awaited && !started && !turn.is_empty() && !self.finished.contains(thread, &turn) {
                if entry.submitted {
                    entry.observed_submission = Some(turn.clone());
                    drop(state);
                    self.end_with(
                        thread,
                        &turn,
                        TurnHow::Failed,
                        Some(message),
                        None,
                        at,
                        None,
                        params.get("error").cloned(),
                    );
                } else {
                    drop(state);
                    self.end_with(
                        thread,
                        &turn,
                        TurnHow::Failed,
                        Some(message),
                        None,
                        at,
                        None,
                        params.get("error").cloned(),
                    );
                }
                return;
            }
            (awaited, started)
        };
        match (awaited, started) {
            (true, false) => self.end_with(
                thread,
                &turn,
                TurnHow::Failed,
                Some(message),
                None,
                at,
                None,
                params.get("error").cloned(),
            ),
            (true, true) => {}
            (false, _) => {}
        }
    }

    /// The app-server stopped: if it is the current one, the turns it ran
    /// end with it, and every thread is resumed with its next words.
    fn exited(&self, server: &Arc<AppServer>, how: String) {
        let _generation = self.generation.lock().expect("never poisoned");
        let (turns, unloaded, facts) = {
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
            let mut facts = Vec::new();
            for (thread, entry) in &mut state.threads {
                entry.loaded = false;
                entry.host_active = false;
                entry.requests.clear();
                if let Some(turn) = entry.turn.take() {
                    entry.started.remove(&turn);
                    self.finished.insert(thread, &turn);
                    entry.unsettled.insert(turn.clone());
                    entry.last_message = None;
                    if let Some(observed) = entry.accounting.ended(&turn) {
                        facts.push((thread.clone(), turn.clone(), observed));
                    }
                    turns.push((thread.clone(), turn));
                }
                entry.started.clear();
                entry.items.clear();
                entry.item_starts.clear();
                entry.last_message = None;
            }
            (turns, state.threads.keys().cloned().collect::<Vec<_>>(), facts)
        };
        (self.log)(format!("the Codex app-server stopped ({how})"));
        let _ = self.events.send(HostEvent::Gone { how, turns, facts });
        for thread in unloaded {
            let _ = self.events.send(HostEvent::Unloaded { thread });
        }
    }
}

/// The most parts a reasoning summary is followed to.
const SUMMARY_PARTS: usize = 256;

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
                if method == "account/chatgptAuthTokens/refresh" && host.assisted.is_some() {
                    let server = server.clone();
                    tokio::spawn(async move {
                        let assisted = host.assisted.as_ref().expect("assisted host");
                        let refreshed = tokio::time::timeout(
                            std::time::Duration::from_secs(9),
                            assisted.refresh(params["previousAccountId"].as_str()),
                        )
                        .await;
                        match refreshed {
                            Ok(Ok(tokens)) => {
                                let _ = server.answer(&id, &tokens);
                            }
                            _ => {
                                let _ = server.refuse(
                                    id,
                                    -32000,
                                    "The original Codex login could not refresh the assisted host's credentials.",
                                );
                            }
                        }
                    });
                    continue;
                }
                if matches!(
                    method.as_str(),
                    "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
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
                let _ = server.refuse(id, -32601, "Fastctx does not answer this request.");
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

/// What the app-server answers for a thread it has not loaded, which some
/// versions say on reading one another version wrote.
fn not_loaded(thread: &str) -> String {
    format!("thread not loaded: {thread}")
}

/// The plugins whose tools work the screen and the browser of the machine
/// Codex runs on, by the name before their marketplace.
const DESKTOP_PLUGINS: [&str; 4] = ["computer-use", "unified-computer-use", "chrome", "browser"];

/// The MCP server the desktop app's Computer Use runs in, the browser's
/// tools included.
const DESKTOP_SERVER: &str = "node_repl";

/// The hands of a login's own configuration that work the screen and the
/// browser of the machine it runs on: its Computer Use server, if it has
/// one, and the plugins of `DESKTOP_PLUGINS`, as that configuration names
/// them.
pub fn desktop_hands(codex_home: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let config = launch::read_config(codex_home)?;
    let servers = config
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .filter(|servers| servers.contains_key(DESKTOP_SERVER))
        .map(|_| vec![DESKTOP_SERVER.to_string()])
        .unwrap_or_default();
    let plugins = config
        .get("plugins")
        .and_then(toml::Value::as_table)
        .map(|plugins| {
            plugins
                .keys()
                .filter(|name| DESKTOP_PLUGINS.contains(&name.split('@').next().unwrap_or_default()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok((servers, plugins))
}

/// Removes the desktop tools when the user disables screen access for a
/// colleague. Full assisted-login isolation is implemented in `assisted`.
pub fn without_desktop(config: &mut Value, servers: &[String], plugins: &[String]) {
    for name in servers {
        config["mcp_servers"][name] = json!({ "enabled": false });
    }
    for name in plugins {
        config["plugins"][name] = json!({ "enabled": false });
    }
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
            "goals": false,
            "send_message_to_user_async": false,
        },
        // `message_user` speaks to the user, asking too, without stopping.
        "tools": { "update_plan": { "enabled": false }, "experimental_request_user_input": { "enabled": false } },
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

/// The World MCP server's identity without overriding native model, permissions,
/// instructions, environment selection or built-in tools.
pub fn native_tools(binary: &Path, env: &[(String,String)], tools: &[&str], budget: usize) -> Value {
    let mut config=world_tools(binary,env,tools,budget);
    json!({"mcp_servers":config["mcp_servers"].take()})
}
