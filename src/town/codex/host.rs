//! One long-lived `codex app-server` per machine, holding every Codex resident as a thread.
//!
//! The binary is the ChatGPT desktop app's own `codex.exe` when the app is installed (its path
//! is read from the user's `~/.codex/config.toml` before every launch, because an app upgrade
//! moves it), launched with the flags the app uses; otherwise a standalone `codex` on PATH.
//! Nothing is modified and no credential is touched: the app-server reads the user's own login.
//!
//! The host turns the protocol into the operations the town needs: start a thread with its
//! first words, send words to a thread, interrupt it, and read its history. Everything the
//! app-server reports about a thread comes out as a `HostEvent`.

use super::rpc::{AppServer, Inbound};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use tokio::sync::mpsc;

/// What a resident's thread is started or resumed with. Kept because `thread/resume` does not
/// remember it, and a thread has to be resumed after every app-server restart.
#[derive(Clone, Debug)]
pub(crate) struct ThreadSpec {
    /// The resident's working directory: its town's bookkeeping directory.
    pub(crate) cwd: PathBuf,
    /// The project and "you work for …", delivered as developer instructions.
    pub(crate) developer_instructions: String,
    /// Per-thread configuration overrides; equivalent to extra `-c` for this thread only.
    pub(crate) config: Value,
}

/// Words for a thread.
#[derive(Clone, Debug)]
pub(crate) enum Input {
    /// Text the model reads as the next user message.
    Text(String),
    /// A fact shaped as the output of a tool, for notices such as a finished command.
    ToolOutput { name: String, namespace: Option<String>, output: String },
}

/// How a send landed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Landed {
    /// The thread was idle; a new turn started.
    NewTurn,
    /// The thread was busy; the words join the running turn at its next tool boundary.
    Steered,
}

/// Everything the town hears about Codex threads.
#[derive(Clone, Debug)]
pub(crate) enum HostEvent {
    TurnStarted { thread: String, turn: String },
    /// An item of a turn started, raw as the app-server sent it.
    ItemStarted { thread: String, item: Value },
    /// A finished item of a turn, raw as the app-server sent it.
    Item { thread: String, item: Value },
    /// A piece of text the model is writing, for live views.
    Delta { thread: String, item: String, delta: String },
    TurnCompleted {
        thread: String,
        turn: String,
        status: String,
        error: Option<Value>,
        duration_ms: Option<u64>,
        /// The last agent message of the turn, verbatim; what goes back to whoever asked.
        last_message: Option<String>,
    },
    /// The app-server asked something the World does not answer; it was declined.
    Declined { thread: Option<String>, method: String },
    /// The app-server stopped; threads come back on the next send.
    ServerGone { reason: String },
}

#[derive(Clone, Debug, Default)]
struct ThreadState {
    spec: Option<ThreadSpec>,
    loaded: bool,
    turn: Option<String>,
    last_message: Option<String>,
}

/// Where the binary comes from and how it is launched.
#[derive(Clone, Debug)]
pub(crate) struct Launch {
    pub(crate) binary: PathBuf,
    pub(crate) args: Vec<String>,
    /// The user's `config.toml` as it was at launch. The app rewrites it when it upgrades
    /// (new binary directory) or restarts (new Computer Use pipe); an app-server started
    /// before that still points at the old ones.
    pub(crate) config: Option<Vec<u8>>,
}

const CLIENT_NAME: &str = "fastctx";
const CLIENT_TITLE: &str = "FastCtx World";

pub(crate) struct CodexHost {
    codex_home: PathBuf,
    workdir: PathBuf,
    log: Arc<dyn Fn(String) + Send + Sync>,
    events: mpsc::UnboundedSender<HostEvent>,
    state: Mutex<HostState>,
    /// Serializes starting the app-server so two sends never launch two of them.
    starting: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct HostState {
    server: Option<(Arc<AppServer>, Launch)>,
    threads: HashMap<String, ThreadState>,
}

impl CodexHost {
    /// `codex_home` is the user's Codex directory (`~/.codex` unless `CODEX_HOME` says
    /// otherwise); `workdir` is where the app-server process itself runs.
    pub(crate) fn new(
        codex_home: PathBuf,
        workdir: PathBuf,
        log: Arc<dyn Fn(String) + Send + Sync>,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<HostEvent>) {
        let (events, receiver) = mpsc::unbounded_channel();
        let host = Arc::new(Self {
            codex_home,
            workdir,
            log,
            events,
            state: Mutex::new(HostState::default()),
            starting: tokio::sync::Mutex::new(()),
        });
        (host, receiver)
    }

    /// Makes a thread the World started earlier known again, after the node restarted.
    pub(crate) fn adopt(&self, thread: &str, spec: ThreadSpec) {
        self.state.lock().threads.entry(thread.to_string()).or_default().spec = Some(spec);
    }

    /// Creates a resident's thread without starting a turn; the caller records the thread id
    /// before sending the first words, so no event of the first turn can outrun it. Returns the
    /// thread id and the model the app-server chose from the user's configuration.
    pub(crate) async fn create_thread(self: &Arc<Self>, spec: ThreadSpec) -> Result<(String, Option<String>), String> {
        let server = self.server().await?;
        let result = server
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
        let thread = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| "thread/start answered without a thread id.".to_string())?
            .to_string();
        let model = result.get("model").and_then(Value::as_str).map(str::to_string);
        self.state.lock().threads.insert(
            thread.clone(),
            ThreadState { spec: Some(spec), loaded: true, ..ThreadState::default() },
        );
        Ok((thread, model))
    }

    /// Sends words to a thread. `turn/start` starts a turn when the thread is idle and folds
    /// the input into the running turn when it is busy; which one happened is read off the
    /// turn id it answers with. A thread the app-server no longer has loaded is resumed first.
    pub(crate) async fn send(self: &Arc<Self>, thread: &str, input: Input) -> Result<(Landed, String), String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        let running = self.state.lock().threads.get(thread).and_then(|state| state.turn.clone());
        let turn = self.turn(&server, thread, &input).await?.ok_or_else(|| "turn/start answered without a turn id.".to_string())?;
        let landed = if running.as_deref() == Some(turn.as_str()) { Landed::Steered } else { Landed::NewTurn };
        Ok((landed, turn))
    }

    /// Stops the running turn of a thread; `false` when none was running.
    pub(crate) async fn interrupt(self: &Arc<Self>, thread: &str) -> Result<bool, String> {
        let turn = self.state.lock().threads.get(thread).and_then(|state| state.turn.clone());
        let Some(turn) = turn else { return Ok(false) };
        let server = self.server().await?;
        server.request("turn/interrupt", json!({"threadId": thread, "turnId": turn})).await?;
        Ok(true)
    }

    /// The thread's recorded history, as the app-server returns it.
    pub(crate) async fn history(self: &Arc<Self>, thread: &str) -> Result<Value, String> {
        let server = self.server().await?;
        server.request("thread/read", json!({"threadId": thread, "includeTurns": true})).await
    }

    /// Whether this thread has a turn running.
    pub(crate) fn is_running(&self, thread: &str) -> bool {
        self.running_turn(thread).is_some()
    }

    /// The id of the turn running in this thread, if one is.
    pub(crate) fn running_turn(&self, thread: &str) -> Option<String> {
        self.state.lock().threads.get(thread).and_then(|state| state.turn.clone())
    }

    /// Starts a turn (or steers the running one) and returns the turn id the server names.
    async fn turn(&self, server: &AppServer, thread: &str, input: &Input) -> Result<Option<String>, String> {
        let mut params = json!({
            "threadId": thread,
            "input": [],
            // Not remembered by the thread: without it the built-in shell and apply_patch return.
            "environments": [],
        });
        match input {
            Input::Text(text) => params["input"] = json!([{"type": "text", "text": text}]),
            Input::ToolOutput { name, namespace, output } => {
                params["toolOutput"] = json!({"name": name, "namespace": namespace, "output": output});
            }
        }
        let result = server.request("turn/start", params).await?;
        let turn = result.pointer("/turn/id").and_then(Value::as_str).map(str::to_string);
        if let Some(turn) = &turn {
            let mut state = self.state.lock();
            let entry = state.threads.entry(thread.to_string()).or_default();
            if entry.turn.as_deref() != Some(turn.as_str()) {
                entry.turn = Some(turn.clone());
                entry.last_message = None;
            }
        }
        Ok(turn)
    }

    async fn ensure_loaded(&self, server: &AppServer, thread: &str) -> Result<(), String> {
        let spec = {
            let state = self.state.lock();
            match state.threads.get(thread) {
                Some(entry) if entry.loaded => return Ok(()),
                Some(entry) => entry.spec.clone(),
                None => None,
            }
        };
        let spec = spec.ok_or_else(|| format!("No Codex resident thread {thread} is known on this machine."))?;
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
        let mut state = self.state.lock();
        let entry = state.threads.entry(thread.to_string()).or_default();
        entry.loaded = true;
        entry.turn = None;
        Ok(())
    }

    /// The running app-server, started on first use or after it stopped. When the user's
    /// `config.toml` changed since it started and no turn is running, it is replaced first, so
    /// threads pick up the app's new binary and Computer Use pipe; a running turn finishes on
    /// the old one.
    async fn server(self: &Arc<Self>) -> Result<Arc<AppServer>, String> {
        let config = read_config(&self.codex_home)?;
        let _starting = self.starting.lock().await;
        {
            let mut state = self.state.lock();
            let busy = state.threads.values().any(|thread| thread.turn.is_some());
            match &state.server {
                Some((server, launch)) if launch.config == config || busy => return Ok(Arc::clone(server)),
                Some((server, _)) => {
                    (self.log)("the user's Codex configuration changed; restarting the app-server".to_string());
                    server.kill();
                    state.server = None;
                }
                None => {}
            }
        }
        let launch = resolve_launch(&self.codex_home)?;
        std::fs::create_dir_all(&self.workdir)
            .map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&self.workdir)))?;
        let (server, inbound) = AppServer::spawn(&launch.binary, &launch.args, &self.workdir, Arc::clone(&self.log))?;
        let init = server
            .request(
                "initialize",
                json!({
                    "clientInfo": {"name": CLIENT_NAME, "title": CLIENT_TITLE, "version": env!("CARGO_PKG_VERSION")},
                    "capabilities": {"experimentalApi": true},
                }),
            )
            .await?;
        server.notify("initialized", json!({})).await?;
        (self.log)(format!(
            "Codex app-server {} started (pid {:?}, {})",
            init.get("userAgent").and_then(Value::as_str).unwrap_or("?"),
            server.pid(),
            crate::paths::display_path(&launch.binary)
        ));
        {
            let mut state = self.state.lock();
            for thread in state.threads.values_mut() {
                thread.loaded = false;
                thread.turn = None;
            }
            state.server = Some((Arc::clone(&server), launch));
        }
        tokio::spawn(pump(Arc::clone(&server), inbound, Arc::downgrade(self)));
        Ok(server)
    }
}

async fn pump(server: Arc<AppServer>, mut inbound: mpsc::UnboundedReceiver<Inbound>, host: Weak<CodexHost>) {
    while let Some(message) = inbound.recv().await {
        let Some(host) = host.upgrade() else { return };
        match message {
            Inbound::Notification { method, params } => {
                if let Some(event) = host.translate(&method, &params) {
                    let _ = host.events.send(event);
                }
            }
            Inbound::Request { id, method, params } => {
                let thread = params.get("threadId").and_then(Value::as_str).map(str::to_string);
                (host.log)(format!("declined the app-server's {method} request"));
                let _ = server
                    .respond(id, Err((-32601, "The World does not answer this request.".to_string())))
                    .await;
                let _ = host.events.send(HostEvent::Declined { thread, method });
            }
            Inbound::Exited(reason) => {
                {
                    let mut state = host.state.lock();
                    // A server replaced on purpose may exit after its successor loaded threads;
                    // only the current one's exit unloads them.
                    let current = match &state.server {
                        Some((current, _)) => Arc::ptr_eq(current, &server),
                        None => true,
                    };
                    if current {
                        state.server = None;
                        for thread in state.threads.values_mut() {
                            thread.loaded = false;
                            thread.turn = None;
                        }
                    }
                }
                (host.log)(reason.clone());
                let _ = host.events.send(HostEvent::ServerGone { reason });
                return;
            }
        }
    }
}

impl CodexHost {
    fn translate(&self, method: &str, params: &Value) -> Option<HostEvent> {
        let thread = params.get("threadId").and_then(Value::as_str)?.to_string();
        match method {
            "turn/started" => {
                let turn = params.pointer("/turn/id").and_then(Value::as_str)?.to_string();
                let mut state = self.state.lock();
                let entry = state.threads.entry(thread.clone()).or_default();
                if entry.turn.as_deref() != Some(turn.as_str()) {
                    entry.turn = Some(turn.clone());
                    entry.last_message = None;
                }
                Some(HostEvent::TurnStarted { thread, turn })
            }
            "item/started" => Some(HostEvent::ItemStarted { thread, item: params.get("item")?.clone() }),
            "item/completed" => {
                let item = params.get("item")?.clone();
                if item.get("type").and_then(Value::as_str) == Some("agentMessage") {
                    let text = item.get("text").and_then(Value::as_str).map(str::to_string);
                    self.state.lock().threads.entry(thread.clone()).or_default().last_message = text;
                }
                Some(HostEvent::Item { thread, item })
            }
            "item/agentMessage/delta" => Some(HostEvent::Delta {
                thread,
                item: params.get("itemId").and_then(Value::as_str).unwrap_or_default().to_string(),
                delta: params.get("delta").and_then(Value::as_str).unwrap_or_default().to_string(),
            }),
            "turn/completed" => {
                let turn = params.get("turn")?;
                let last_message = {
                    let mut state = self.state.lock();
                    let entry = state.threads.entry(thread.clone()).or_default();
                    entry.turn = None;
                    entry.last_message.take()
                };
                Some(HostEvent::TurnCompleted {
                    thread,
                    turn: turn.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    status: turn.get("status").and_then(Value::as_str).unwrap_or("unknown").to_string(),
                    error: turn.get("error").filter(|error| !error.is_null()).cloned(),
                    duration_ms: turn.get("durationMs").and_then(Value::as_u64),
                    last_message,
                })
            }
            _ => None,
        }
    }
}

/// Finds the Codex binary to launch. The desktop app records its own `codex.exe` in the
/// user's config (`[mcp_servers.node_repl.env] CODEX_CLI_PATH`); that one lends the app's
/// Computer Use, Chrome plugin and connectors to every thread. Without the app, a standalone
/// `codex` on PATH speaks the same protocol.
pub(crate) fn resolve_launch(codex_home: &Path) -> Result<Launch, String> {
    if let Some(binary) = app_binary(codex_home)? {
        return Ok(Launch {
            binary,
            args: vec![
                "-c".to_string(),
                "features.code_mode_host=true".to_string(),
                // Both Computer Use services name their tool `js` and Codex keeps whichever
                // registers first; keep only the Windows-native one.
                "-c".to_string(),
                "plugins.unified-computer-use@openai-bundled.mcp_servers.cua_repl.enabled=false".to_string(),
                "app-server".to_string(),
            ],
            config: read_config(codex_home)?,
        });
    }
    let binary = find_on_path(if cfg!(windows) { "codex.exe" } else { "codex" }).ok_or_else(|| {
        "Codex is not installed on this machine: neither the ChatGPT desktop app nor a codex command was found.".to_string()
    })?;
    Ok(Launch { binary, args: vec!["app-server".to_string()], config: read_config(codex_home)? })
}

fn read_config(codex_home: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = codex_home.join("config.toml");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read {}: {error}", crate::paths::display_path(&path))),
    }
}

fn app_binary(codex_home: &Path) -> Result<Option<PathBuf>, String> {
    let path = codex_home.join("config.toml");
    let Some(bytes) = read_config(codex_home)? else { return Ok(None) };
    let text = String::from_utf8(bytes).map_err(|error| format!("{} is not UTF-8: {error}", crate::paths::display_path(&path)))?;
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("Cannot parse {}: {error}", crate::paths::display_path(&path)))?;
    let binary = document
        .get("mcp_servers")
        .and_then(|servers| servers.get("node_repl"))
        .and_then(|server| server.get("env"))
        .and_then(|env| env.get("CODEX_CLI_PATH"))
        .and_then(|value| value.as_str())
        .map(PathBuf::from);
    Ok(binary.filter(|binary| binary.is_file()))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|directory| directory.join(name)).find(|candidate| candidate.is_file())
}
