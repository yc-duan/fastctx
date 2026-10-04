//! Codex residents: threads of the one long-lived `codex app-server` this
//! machine's node holds (`world-colleagues.md`, 世界怎样拉起 GPT 同事).
//!
//! The host turns the app-server's protocol into what the World does with a
//! session: start a thread, get it ready for words, send it words, stop its
//! turn, read its history. What the app-server reports about a thread comes
//! out as session events (`fastctx_types::session`); what Codex's
//! notifications mean is known here and nowhere else.
//!
//! Codex writes no compatibility promise for this protocol. Only fields it
//! does not mark experimental are read, and a notification, item or status
//! not known here is passed on as it is or left alone, never guessed at.
//! One experimental field is used, `environments: []`, which takes away a
//! thread's own shell and file editing; it must be sent with every turn, as
//! the thread does not keep it. If Codex changes it, starting a turn fails
//! loudly.

mod items;
pub mod launch;
mod server;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use fastctx_types::session::{Happened, Health, HealthKind, SessionEvent, TurnEnd, TurnHow};
use jiff::Timestamp;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

use launch::Launch;
use server::{AppServer, Inbound};

pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// How long Codex waits for a thread's tool server to start; the World
/// gives Codex this, and waits as long for word of how it went.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// How long Codex waits for one call of a World tool. A `run` answers
/// within its fifteen seconds and a write or a colleague's request at once;
/// what can take longer is a large result travelling between machines.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(300);

/// The output limit Codex is given for a tool whose answers are at most
/// `budget` characters, so Codex never cuts an answer after the World has
/// sized it (`world-view.md`, 一次给多少). Codex counts a fourth of an
/// answer's UTF-8 bytes and allows 1.2 times the limit; a character is at
/// most four bytes, so `budget` characters count at most `budget`, and the
/// limit is `budget / 1.2`.
pub fn output_limit(budget: usize) -> u64 {
    budget as u64 * 10 / 12
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
}

/// Words for a thread.
#[derive(Clone, Debug)]
pub enum Input {
    /// What the model reads as the next message to it.
    Text(String),
    /// A fact shaped as a tool's output, such as a job's end.
    ToolOutput { name: String, namespace: Option<String>, output: String },
}

/// Where words went: a turn that started with them, or the running turn,
/// which they join at its next tool boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    pub turn: String,
    pub joined: bool,
}

/// What the host tells the node.
#[derive(Clone, Debug)]
pub enum HostEvent {
    Happened {
        thread: String,
        event: SessionEvent,
    },
    /// The app-server stopped, and these turns, as (thread, turn), with it;
    /// `how` is how it exited, such as `exit code: 1`. Threads come back
    /// with the next words.
    Gone {
        how: String,
        turns: Vec<(String, String)>,
    },
}

pub struct CodexHost {
    codex_home: PathBuf,
    workdir: PathBuf,
    env: Vec<(OsString, OsString)>,
    log: Log,
    events: mpsc::UnboundedSender<HostEvent>,
    state: Mutex<State>,
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
    server: Option<(Arc<AppServer>, Launch)>,
    threads: HashMap<String, Thread>,
}

#[derive(Default)]
struct Thread {
    spec: Option<ThreadSpec>,
    loaded: bool,
    /// The turn running now: reported started, or named in the answer to
    /// the words that started it.
    turn: Option<String>,
    /// Turns reported started and not yet ended. A turn the app-server took
    /// but could not set up never starts, and the error it reports is its
    /// only end.
    started: HashSet<String>,
    /// Turns that ended: an answer to `turn/start` that arrives after its
    /// turn ended must not make it run again.
    finished: HashSet<String>,
    /// Errors for turns not yet named by the answer that started them.
    failed_early: HashMap<String, String>,
    /// The agent's last message in the running turn.
    last_message: Option<String>,
    /// The app-server said how the thread's World tools started since it was
    /// last started or resumed.
    tools_settled: bool,
    /// Why the World's tools are not there in the thread's current load.
    tools_missing: Option<String>,
}

impl CodexHost {
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
    /// any event of its first turn. Returns the thread and the model Codex
    /// chose from the user's configuration.
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
        // What the app-server said of the thread, how its tools started
        // among it, may have come before this answer.
        let mut state = self.lock();
        let entry = state.threads.entry(thread.clone()).or_default();
        entry.spec = Some(spec);
        entry.loaded = true;
        Ok((thread, model))
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
    pub async fn send(self: &Arc<Self>, thread: &str, input: Input) -> Result<Sent, String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        let mut params = json!({
            "threadId": thread,
            "input": [],
            // The thread does not keep it: without it its own shell and file
            // editing come back.
            "environments": [],
        });
        match input {
            Input::Text(text) => params["input"] = json!([{ "type": "text", "text": text }]),
            Input::ToolOutput { name, namespace, output } => {
                params["toolOutput"] = json!({ "name": name, "namespace": namespace, "output": output })
            }
        }
        let answer = server.request("turn/start", params).await?;
        let turn = answer
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or("turn/start answered without a turn id")?
            .to_string();
        let (joined, failed) = {
            let mut state = self.lock();
            // An app-server that stopped, or was replaced, while it answered
            // runs nothing more; its turns ended with it.
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server)) {
                return Err("the Codex app-server stopped as it took the words; they went into no turn".to_string());
            }
            let entry = state.threads.entry(thread.to_string()).or_default();
            let joined = entry.turn.as_deref() == Some(turn.as_str());
            if !joined && !entry.finished.contains(&turn) {
                entry.turn = Some(turn.clone());
                entry.last_message = None;
            }
            (joined, entry.failed_early.remove(&turn))
        };
        if let Some(reason) = failed {
            self.end(thread, &turn, TurnHow::Failed, Some(reason), None, Timestamp::now());
        }
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
        let answer = server.request("thread/read", json!({ "threadId": thread, "includeTurns": true })).await?;
        let running = self.running_turn(thread);
        Ok(history(&answer, running.as_deref()))
    }

    pub fn running_turn(&self, thread: &str) -> Option<String> {
        self.lock().threads.get(thread).and_then(|entry| entry.turn.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("never poisoned")
    }

    async fn ensure_loaded(&self, server: &AppServer, thread: &str) -> Result<(), String> {
        if self.lock().threads.get(thread).is_some_and(|entry| entry.loaded) {
            return Ok(());
        }
        let _loading = self.loading.lock().await;
        let spec = {
            let mut state = self.lock();
            let Some(entry) = state.threads.get_mut(thread) else {
                return Err(format!("no Codex thread {thread} is known on this machine"));
            };
            if entry.loaded {
                return Ok(());
            }
            // Resuming starts the thread's tool servers again; what they say
            // from here on counts.
            entry.tools_settled = false;
            entry.tools_missing = None;
            entry.spec.clone().ok_or_else(|| format!("how the Codex thread {thread} was started is not known"))?
        };
        server
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
        let entry = state.threads.entry(thread.to_string()).or_default();
        entry.loaded = true;
        entry.turn = None;
        Ok(())
    }

    /// The running app-server, started on first use or after it stopped.
    /// When the user's configuration changed since it started and no turn
    /// runs, it is replaced first, so threads take up the app's new binary
    /// and Computer Use pipe; a running turn finishes on the old one.
    async fn server(self: &Arc<Self>) -> Result<Arc<AppServer>, String> {
        let config = launch::read_config(&self.codex_home)?;
        let _starting = self.starting.lock().await;
        {
            let mut state = self.lock();
            let busy = state.threads.values().any(|thread| thread.turn.is_some());
            match &state.server {
                Some((server, launch)) if launch.config == config || busy => return Ok(server.clone()),
                Some((server, _)) => {
                    (self.log)("the user's Codex configuration changed; restarting the app-server".to_string());
                    server.kill();
                    state.server = None;
                }
                None => {}
            }
        }
        let path = self.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("PATH")).map(|(_, value)| value);
        let launch = launch::resolve(&self.codex_home, path)?;
        std::fs::create_dir_all(&self.workdir)
            .map_err(|error| format!("making {}: {error}", self.workdir.display()))?;
        let (server, inbound) =
            AppServer::spawn(&launch.binary, &launch.args, &self.workdir, &self.env, self.log.clone())
                .map_err(|error| format!("starting {}: {error}", launch.binary.display()))?;
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
        for thread in state.threads.values_mut() {
            thread.loaded = false;
            thread.turn = None;
        }
        state.server = Some((server.clone(), launch));
        Ok(server)
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
        let last_message = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            entry.finished.insert(turn.to_string());
            entry.started.remove(turn);
            if entry.turn.as_deref() == Some(turn) {
                entry.turn = None;
            }
            entry.last_message.take()
        };
        let end = TurnEnd { how, reason, duration_ms, last_message };
        self.emit(thread, at, Happened::TurnEnded { turn: turn.to_string(), end });
    }

    fn emit(&self, thread: &str, at: Timestamp, happened: Happened) {
        let event = SessionEvent { at, happened };
        let _ = self.events.send(HostEvent::Happened { thread: thread.to_string(), event });
    }

    fn health(&self, thread: &str, turn: Option<String>, kind: HealthKind, text: String) {
        self.emit(thread, Timestamp::now(), Happened::Health { turn, health: Health { kind, text } });
    }

    /// Reads one notification about a thread.
    fn notified(&self, method: &str, params: &Value) {
        let Some(thread) = params.get("threadId").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        let text = |pointer: &str| params.pointer(pointer).and_then(Value::as_str).map(str::to_string);
        let turn = text("/turnId").or_else(|| self.running_turn(&thread));
        match method {
            "turn/started" => {
                let Some(turn) = text("/turn/id") else { return };
                {
                    let mut state = self.lock();
                    let entry = state.threads.entry(thread.clone()).or_default();
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
                let (id, read) = items::read(item);
                let happened = match (read, completed) {
                    (items::Item::Message { text }, true) => {
                        self.lock().threads.entry(thread.clone()).or_default().last_message = Some(text.clone());
                        Happened::Said { turn, item: id, text }
                    }
                    (items::Item::Tool { tool, detail, .. }, false) => {
                        Happened::ToolStarted { turn, item: id, tool, detail }
                    }
                    (items::Item::Tool { tool, detail, failed }, true) => {
                        Happened::ToolEnded { turn, item: id, tool, detail, failed }
                    }
                    // Words that went in are told by whoever sent them, who
                    // knows who they are from.
                    _ => return,
                };
                self.emit(&thread, at, happened);
            }
            "item/agentMessage/delta" => {
                let (Some(turn), Some(item), Some(delta)) = (turn, text("/itemId"), text("/delta")) else { return };
                self.emit(&thread, Timestamp::now(), Happened::Writing { turn, item, delta });
            }
            "turn/completed" => {
                let Some(id) = text("/turn/id") else { return };
                let how = turn_how(text("/turn/status").as_deref());
                let reason = text("/turn/error/message");
                let duration = params.pointer("/turn/durationMs").and_then(Value::as_u64);
                let at = seconds(params.pointer("/turn/completedAt"));
                self.end(&thread, &id, how, reason, duration, at);
            }
            "error" => self.error(&thread, params),
            "model/safetyBuffering/updated" => {
                let list = |key: &str| {
                    let words: Vec<&str> = params
                        .get(key)
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    if words.is_empty() { "not given".to_string() } else { words.join(", ") }
                };
                let text = format!(
                    "OpenAI is holding this turn's output for extra safety processing (use: {}; reason: {})",
                    list("useCases"),
                    list("reasons")
                );
                self.health(&thread, turn, HealthKind::Safety, text);
            }
            "model/rerouted" => {
                let text = format!(
                    "OpenAI moved this turn from {} to {} ({})",
                    text("/fromModel").unwrap_or_default(),
                    text("/toModel").unwrap_or_default(),
                    text("/reason").unwrap_or_default()
                );
                self.health(&thread, turn, HealthKind::Rerouted, text);
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
                        let entry = state.threads.entry(thread).or_default();
                        entry.tools_settled = matches!(status.as_str(), "ready" | "failed" | "cancelled");
                        entry.tools_missing = missing;
                    }
                    self.tools_changed.notify_waiters();
                } else if status == "failed" {
                    self.health(
                        &thread,
                        turn,
                        HealthKind::ToolServer,
                        format!("its {name} tools did not start: {why}"),
                    );
                }
            }
            "warning" => {
                let text = format!("Codex warns: {}", text("/message").unwrap_or_default());
                self.health(&thread, turn, HealthKind::Warning, text);
            }
            "thread/closed" => {
                // Unloaded: the next words resume it.
                if let Some(entry) = self.lock().threads.get_mut(&thread) {
                    entry.loaded = false;
                }
            }
            _ => {}
        }
    }

    /// An error notification. With `willRetry` it is a reconnect and the turn
    /// goes on. Otherwise, an error in a started turn is followed by the
    /// turn's end, which carries it; for a turn that was taken but never
    /// started it is that turn's only end, and may come before the answer to
    /// `turn/start` that names the turn.
    fn error(&self, thread: &str, params: &Value) {
        let message = params.pointer("/error/message").and_then(Value::as_str).unwrap_or_default().to_string();
        let turn = params.get("turnId").and_then(Value::as_str).unwrap_or_default().to_string();
        if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
            let text = format!("Codex is reconnecting to the model: {message}");
            self.health(thread, Some(turn).filter(|turn| !turn.is_empty()), HealthKind::Reconnecting, text);
            return;
        }
        let (awaited, started) = {
            let mut state = self.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            let awaited = entry.turn.as_deref() == Some(turn.as_str());
            let started = entry.started.contains(&turn);
            if !awaited && !started && !turn.is_empty() && !entry.finished.contains(&turn) {
                entry.failed_early.insert(turn, message);
                return;
            }
            (awaited, started)
        };
        match (awaited, started) {
            (true, false) => self.end(thread, &turn, TurnHow::Failed, Some(message), None, Timestamp::now()),
            (true, true) => {}
            (false, _) => self.health(thread, None, HealthKind::Error, format!("Codex reported an error: {message}")),
        }
    }

    /// The app-server stopped: if it is the current one, the turns it ran
    /// end with it, and every thread is resumed with its next words.
    fn exited(&self, server: &Arc<AppServer>, how: String) {
        let turns = {
            let mut state = self.lock();
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, server)) {
                // One replaced on purpose exits after its successor may have
                // started turns; those are not its own.
                (self.log)(format!("a replaced Codex app-server stopped ({how})"));
                return;
            }
            state.server = None;
            let mut turns = Vec::new();
            for (thread, entry) in &mut state.threads {
                entry.loaded = false;
                if let Some(turn) = entry.turn.take() {
                    entry.started.remove(&turn);
                    entry.finished.insert(turn.clone());
                    entry.last_message = None;
                    turns.push((thread.clone(), turn));
                }
            }
            turns
        };
        (self.log)(format!("the Codex app-server stopped ({how})"));
        let _ = self.events.send(HostEvent::Gone { how, turns });
    }
}

/// Reads what the app-server says, until it stops.
async fn pump(server: Arc<AppServer>, mut inbound: mpsc::UnboundedReceiver<Inbound>, host: Weak<CodexHost>) {
    while let Some(message) = inbound.recv().await {
        let Some(host) = host.upgrade() else { return };
        match message {
            Inbound::Notification { method, params } => host.notified(&method, &params),
            Inbound::Request { id, method, .. } => {
                // The World answers nothing the app-server asks: it gives no
                // approvals, and its threads ask for none.
                (host.log)(format!("declined the app-server's {method} request"));
                let _ = server.refuse(id, -32601, "The World does not answer this request.");
            }
            Inbound::Exited(reason) => {
                host.exited(&server, reason);
                return;
            }
        }
    }
}

/// A thread's history from `thread/read`, as session events.
fn history(answer: &Value, running: Option<&str>) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    for turn in answer.pointer("/thread/turns").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = turn.get("id").and_then(Value::as_str) else { continue };
        let started = seconds(turn.get("startedAt"));
        let mut push = |at, happened| events.push(SessionEvent { at, happened });
        push(started, Happened::TurnStarted { turn: id.to_string() });
        let mut last_message = None;
        for item in turn.get("items").and_then(Value::as_array).into_iter().flatten() {
            let (item_id, read) = items::read(item);
            let turn = id.to_string();
            match read {
                items::Item::Words { text } => push(started, Happened::Heard { turn: Some(turn), from: None, text }),
                items::Item::Message { text } => {
                    last_message = Some(text.clone());
                    push(started, Happened::Said { turn, item: item_id, text });
                }
                items::Item::Tool { tool, detail, failed } => {
                    push(
                        started,
                        Happened::ToolStarted {
                            turn: turn.clone(),
                            item: item_id.clone(),
                            tool: tool.clone(),
                            detail: detail.clone(),
                        },
                    );
                    push(started, Happened::ToolEnded { turn, item: item_id, tool, detail, failed });
                }
                items::Item::Quiet => {}
            }
        }
        let status = turn.get("status").and_then(Value::as_str);
        // The record says a turn that is still running was interrupted
        // until it has been written to the end.
        if status == Some("inProgress") || running == Some(id) {
            continue;
        }
        let end = TurnEnd {
            how: turn_how(status),
            reason: turn.pointer("/error/message").and_then(Value::as_str).map(str::to_string),
            duration_ms: turn.get("durationMs").and_then(Value::as_u64),
            last_message,
        };
        push(seconds(turn.get("completedAt")), Happened::TurnEnded { turn: id.to_string(), end });
    }
    events
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
/// any fastctx the user registered: `binary` serving with `apply_patch`,
/// with the environment `env`, and the limits above. A thread's settings
/// merge into the user's table by table, so every setting of the server
/// that changes what it offers is given here, lists replacing the user's
/// whole.
pub fn world_tools(binary: &Path, env: &[(String, String)], tools: &[&str], budget: usize) -> Value {
    let env: serde_json::Map<String, Value> =
        env.iter().map(|(name, value)| (name.clone(), Value::String(value.clone()))).collect();
    let limit = output_limit(budget);
    json!({
        "tool_output_token_limit": limit,
        "mcp_servers": { SERVER_NAME: {
            "command": binary,
            "args": ["serve", "--writer", "apply_patch"],
            "env": env,
            "env_vars": [],
            "enabled": true,
            "enabled_tools": tools,
            "disabled_tools": [],
            "omit_tools_from": [],
            "startup_timeout_sec": STARTUP_TIMEOUT.as_secs(),
            "tool_timeout_sec": TOOL_TIMEOUT.as_secs(),
            "tools": { "run": { "output_token_limit": limit } },
        }},
    })
}
