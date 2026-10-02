//! The town core on one machine: who lives here, what they do, and the words on their way to
//! them.
//!
//! It is driven from three sides: the control center registers every MCP session as a resident
//! and reports its hands' actions; the resident tools ask for colleagues and speak to them; the
//! Codex host reports what its threads do. Words for a resident go out the way its host can
//! take them (`deliver`), and what cannot be delivered right now waits in a durable queue in
//! the resident's directory.

use super::codex::host::{CodexHost, HostEvent, Input, Landed, ThreadSpec};
use super::records::{About, Birth, MessageLine, Now, Records, new_resident_id};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// Something on its way to a resident.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Delivery {
    /// Unique id, also the file name while it waits.
    pub(crate) id: String,
    /// Who it is from: a resident id, `world`, or `user`.
    pub(crate) from: String,
    /// `answer`, `message`, `notice`.
    pub(crate) kind: String,
    /// The text the resident reads, head note included.
    pub(crate) text: String,
    /// Shape it as a tool output for hosts that support it (Codex), for facts such as a
    /// finished command; a reply to read is text.
    #[serde(default)]
    pub(crate) as_tool_output: bool,
}

/// How a resident can be reached.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Route {
    /// A thread in this machine's Codex app-server.
    CodexThread(String),
    /// A Claude session; once its mod has registered, the session id whose inbox it reads.
    ClaudeMod(Option<String>),
    /// No channel: words ride on its next World tool response.
    Piggyback,
}

struct Live {
    records: Records,
    about: About,
    route: Route,
    /// The host's per-session inbox address, which pairs a Claude session's MCP connection
    /// with its mod's registration.
    inbox_address: Option<String>,
    /// MCP sessions currently attached for this resident.
    sessions: u32,
    /// Residents whose words went into the running turn; they get its last message.
    readers: BTreeSet<String>,
    turn_started: Option<Instant>,
}

#[derive(Default)]
struct State {
    residents: HashMap<String, Live>,
    /// Codex thread id → resident id.
    threads: HashMap<String, String>,
}

/// Settings that do not change while the node runs.
#[derive(Clone)]
pub(crate) struct TownConfig {
    /// This machine's name in the World.
    pub(crate) machine: String,
    /// `~/.fastctx/world`.
    pub(crate) world_dir: PathBuf,
    /// The fastctx binary residents launched by the World use as their MCP server.
    pub(crate) fastctx: PathBuf,
}

pub(crate) struct Town {
    config: TownConfig,
    codex: Arc<CodexHost>,
    log: Arc<dyn Fn(String) + Send + Sync>,
    state: Mutex<State>,
}

/// What `ask_colleague` hands back.
pub(crate) struct Asked {
    pub(crate) id: String,
    pub(crate) machine: String,
}

impl Town {
    pub(crate) fn new(
        config: TownConfig,
        codex: Arc<CodexHost>,
        events: mpsc::UnboundedReceiver<HostEvent>,
        log: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Arc<Self> {
        let town = Arc::new(Self { config, codex, log, state: Mutex::new(State::default()) });
        town.restore();
        tokio::spawn(Arc::clone(&town).follow_codex(events));
        tokio::spawn(Arc::clone(&town).follow_claude());
        town
    }

    pub(crate) fn log(&self, line: String) {
        (self.log)(line);
    }

    pub(crate) fn machine(&self) -> &str {
        &self.config.machine
    }

    /// The directory holding a resident's records.
    pub(crate) fn resident_dir(&self, town: Option<&str>, id: &str) -> PathBuf {
        match town {
            Some(town) => self.config.world_dir.join("towns").join(town).join("residents").join(id),
            None => self.config.world_dir.join("outside").join("residents").join(id),
        }
    }

    /// Whether any resident ever recorded on this machine had this id, in or outside a town.
    fn recorded(&self, id: &str) -> bool {
        self.resident_dir(None, id).exists()
            || subdirectories(&self.config.world_dir.join("towns")).iter().any(|town| town.join("residents").join(id).exists())
    }

    /// A town's bookkeeping directory: where residents the World launches stand.
    pub(crate) fn town_dir(&self, town: &str) -> PathBuf {
        self.config.world_dir.join("towns").join(town)
    }

    /// Registers an MCP session as a resident. A session the World launched names itself in
    /// `FASTCTX_RESIDENT`; any other session becomes a new resident born of the user.
    pub(crate) fn attach(
        &self,
        agent: &str,
        agent_version: Option<&str>,
        named: Option<&str>,
        surface: &str,
        inbox_address: Option<&str>,
    ) -> Result<String, String> {
        let mut state = self.state.lock();
        if let Some(id) = named
            && let Some(live) = state.residents.get_mut(id)
        {
            live.sessions += 1;
            return Ok(id.to_string());
        }
        let id = match named {
            Some(id) => id.to_string(),
            None => new_resident_id(agent, |candidate| state.residents.contains_key(candidate) || self.recorded(candidate))?,
        };
        let about = About {
            id: id.clone(),
            agent: agent.to_string(),
            agent_version: agent_version.map(str::to_string),
            model: None,
            model_source: None,
            machine: self.config.machine.clone(),
            surface: surface.to_string(),
            birth: Birth::User,
            since: crate::world::now_rfc3339(),
            session: None,
            town: None,
        };
        let records = Records::new(self.resident_dir(None, &id));
        records.create()?;
        records.write_about(&about)?;
        records.write_now(&Now { state: "unknown".into(), since: about.since.clone(), ..Now::default() })?;
        let route = if agent.to_ascii_lowercase().contains("claude") { Route::ClaudeMod(None) } else { Route::Piggyback };
        state.residents.insert(
            id.clone(),
            Live {
                records,
                about,
                route,
                inbox_address: inbox_address.map(str::to_string),
                sessions: 1,
                readers: BTreeSet::new(),
                turn_started: None,
            },
        );
        drop(state);
        self.pair_claude_sessions();
        Ok(id)
    }

    /// An MCP session ended. A resident with no session left has left, unless the World holds
    /// its thread, in which case it is only asleep.
    pub(crate) fn detach(&self, id: &str) {
        let mut state = self.state.lock();
        let Some(live) = state.residents.get_mut(id) else { return };
        live.sessions = live.sessions.saturating_sub(1);
        if live.sessions == 0 && !matches!(live.route, Route::CodexThread(_)) {
            let _ = live.records.write_now(&Now { state: "left".into(), since: crate::world::now_rfc3339(), ..Now::default() });
        }
    }

    /// Records one action of a resident's hands and makes it the last thing it did.
    pub(crate) fn record_action(&self, id: &str, machine: &str, hand: &str, what: &str, result: &str) {
        let state = self.state.lock();
        let Some(live) = state.residents.get(id) else { return };
        let at = crate::world::now_rfc3339();
        match live.records.append_action(&at, machine, hand, what, result) {
            Ok(_) => {
                let mut now = live.records.now().ok().flatten().unwrap_or_default();
                now.last_action = Some(format!("{hand} {what}"));
                now.last_action_at = Some(at);
                let _ = live.records.write_now(&now);
            }
            Err(error) => (self.log)(format!("cannot record an action of {id}: {error}")),
        }
    }

    /// Starts a Codex colleague for `requester` on this machine, in the requester's town.
    pub(crate) async fn ask_colleague(&self, requester: &str, prompt: &str, label: &str) -> Result<Asked, String> {
        let town = {
            let state = self.state.lock();
            let live = state.residents.get(requester).ok_or_else(|| format!("{requester} is not a resident of this machine."))?;
            live.about.town.clone()
        };
        let id = {
            let state = self.state.lock();
            new_resident_id("codex", |candidate| state.residents.contains_key(candidate) || self.recorded(candidate))?
        };
        let records = Records::new(self.resident_dir(town.as_deref(), &id));
        records.create()?;
        let about = About {
            id: id.clone(),
            agent: "codex".to_string(),
            agent_version: None,
            model: None,
            model_source: None,
            machine: self.config.machine.clone(),
            surface: "world".to_string(),
            birth: Birth::Colleague { requester: requester.to_string(), label: label.to_string() },
            since: crate::world::now_rfc3339(),
            session: None,
            town: town.clone(),
        };
        records.write_about(&about)?;
        records.write_now(&Now { state: "busy".into(), since: about.since.clone(), ..Now::default() })?;
        records.append_message(&MessageLine {
            at: about.since.clone(),
            kind: "asked".into(),
            peer: requester.to_string(),
            turn: None,
            text: Some(prompt.to_string()),
        })?;
        let spec = self.colleague_spec(&id, requester, town.as_deref());
        {
            let mut state = self.state.lock();
            state.residents.insert(
                id.clone(),
                Live {
                    records: records.clone(),
                    about: about.clone(),
                    route: Route::Piggyback,
                    inbox_address: None,
                    sessions: 0,
                    readers: BTreeSet::from([requester.to_string()]),
                    turn_started: Some(Instant::now()),
                },
            );
        }
        let failed = |error: String| {
            self.state.lock().residents.remove(&id);
            let _ = records.write_now(&Now { state: "error".into(), since: crate::world::now_rfc3339(), ..Now::default() });
            error
        };
        let thread = self.codex.create_thread(spec).await.map_err(failed)?;
        let mut about = about;
        about.session = Some(thread.clone());
        records.write_about(&about)?;
        {
            let mut state = self.state.lock();
            state.threads.insert(thread.clone(), id.clone());
            if let Some(live) = state.residents.get_mut(&id) {
                live.about = about;
                live.route = Route::CodexThread(thread.clone());
            }
        }
        let first = format!("=== from {requester} ===\n{prompt}");
        self.codex.send(&thread, Input::Text(first)).await.map_err(failed)?;
        Ok(Asked { id, machine: self.config.machine.clone() })
    }

    /// What a colleague's thread is started and resumed with.
    fn colleague_spec(&self, id: &str, requester: &str, town: Option<&str>) -> ThreadSpec {
        ThreadSpec {
            cwd: match town {
                Some(town) => self.town_dir(town),
                None => self.config.world_dir.join("outside"),
            },
            developer_instructions: colleague_instructions(id, requester, town, &self.config.machine),
            config: json!({
                "mcp_servers": {"fastctx": {
                    "command": self.config.fastctx,
                    "args": ["serve"],
                    "env": {"FASTCTX_RESIDENT": id, "FASTCTX_COLLEAGUE": "1"},
                }},
            }),
        }
    }

    /// Takes back what this machine's town held before the node restarted. A Codex colleague
    /// sleeps in its thread and wakes at the next words; a turn it was running died with the
    /// old app-server. Every other resident's session ended with the old node.
    fn restore(&self) {
        let mut dirs = Vec::new();
        for parent in [self.config.world_dir.join("outside").join("residents")]
            .into_iter()
            .chain(subdirectories(&self.config.world_dir.join("towns")).into_iter().map(|town| town.join("residents")))
        {
            dirs.extend(subdirectories(&parent));
        }
        let mut state = self.state.lock();
        for dir in dirs {
            let records = Records::new(dir);
            let about = match records.about() {
                Ok(Some(about)) if about.machine == self.config.machine => about,
                Ok(_) => continue,
                Err(error) => {
                    (self.log)(error);
                    continue;
                }
            };
            let now = records.now().ok().flatten().unwrap_or_default();
            let thread = match (&about.birth, about.agent.as_str(), &about.session) {
                (Birth::Colleague { requester, .. }, "codex", Some(thread)) => {
                    self.codex.adopt(thread, self.colleague_spec(&about.id, requester, about.town.as_deref()));
                    Some(thread.clone())
                }
                _ => None,
            };
            let state_now = match (&thread, now.state.as_str()) {
                (Some(_), "busy") => Some("interrupted"),
                (Some(_), _) => None,
                (None, "left") => None,
                (None, _) => Some("left"),
            };
            if let Some(word) = state_now {
                let _ = records.write_now(&Now { state: word.into(), since: crate::world::now_rfc3339(), ..now });
            }
            let Some(thread) = thread else { continue };
            state.threads.insert(thread.clone(), about.id.clone());
            state.residents.insert(
                about.id.clone(),
                Live {
                    records,
                    about,
                    route: Route::CodexThread(thread),
                    inbox_address: None,
                    sessions: 0,
                    readers: BTreeSet::new(),
                    turn_started: None,
                },
            );
        }
    }

    /// Speaks to a resident on this machine on behalf of `from`. For a colleague, `from` then
    /// receives the last message of the turn its words went into.
    pub(crate) async fn speak(&self, from: &str, to: &str, message: &str) -> Result<Spoken, String> {
        let route = {
            let state = self.state.lock();
            let live = state.residents.get(to).ok_or_else(|| format!("No resident named {to} lives on {}.", self.config.machine))?;
            let _ = live.records.append_message(&MessageLine {
                at: crate::world::now_rfc3339(),
                kind: "said".into(),
                peer: from.to_string(),
                turn: None,
                text: Some(message.to_string()),
            });
            live.route.clone()
        };
        let text = format!("=== from {from} ===\n{message}");
        match route {
            Route::CodexThread(thread) => {
                let landed = self.codex.send(&thread, Input::Text(text)).await?;
                let mut state = self.state.lock();
                if let Some(live) = state.residents.get_mut(to) {
                    if landed == Landed::NewTurn {
                        live.readers.clear();
                        live.turn_started = Some(Instant::now());
                    }
                    live.readers.insert(from.to_string());
                    let _ = live.records.write_now(&Now { state: "busy".into(), since: crate::world::now_rfc3339(), ..Now::default() });
                }
                Ok(match landed {
                    Landed::NewTurn => Spoken::NewTurn,
                    Landed::Steered => Spoken::JoinedTurn,
                })
            }
            Route::ClaudeMod(_) | Route::Piggyback => {
                let delivery = Delivery { id: delivery_id()?, from: from.to_string(), kind: "message".into(), text, as_tool_output: false };
                self.deliver(to, delivery).await?;
                Ok(Spoken::Queued)
            }
        }
    }

    /// Delivers words to a resident on this machine by whatever route its host offers.
    pub(crate) async fn deliver(&self, to: &str, delivery: Delivery) -> Result<(), String> {
        let route = self.state.lock().residents.get(to).map(|live| live.route.clone());
        match route {
            Some(Route::CodexThread(thread)) => {
                let input = if delivery.as_tool_output {
                    Input::ToolOutput { name: "world".into(), namespace: Some("fastctx".into()), output: delivery.text }
                } else {
                    Input::Text(delivery.text)
                };
                self.codex.send(&thread, input).await.map(|_| ())
            }
            Some(Route::ClaudeMod(Some(session))) => super::claude::deliver(&self.config.world_dir, &session, &mod_delivery(delivery)),
            Some(Route::ClaudeMod(None) | Route::Piggyback) => self.enqueue(to, delivery),
            None => Err(format!("No resident named {to} lives on {}.", self.config.machine)),
        }
    }

    /// Links Claude residents to the sessions their mods registered: by the resident id the
    /// World gave a session it launched, or else by the session's inbox address. Whatever
    /// waited for a resident before its pairing moves to its session's inbox.
    fn pair_claude_sessions(&self) {
        let mut paired = Vec::new();
        {
            let mut state = self.state.lock();
            if !state.residents.values().any(|live| live.route == Route::ClaudeMod(None)) {
                return;
            }
            for session in super::claude::sessions(&self.config.world_dir) {
                let registration = match super::claude::registration(&self.config.world_dir, &session) {
                    Ok(Some(registration)) => registration,
                    Ok(None) => continue,
                    Err(error) => {
                        (self.log)(error);
                        continue;
                    }
                };
                let found = state.residents.iter_mut().find(|(id, live)| {
                    live.route == Route::ClaudeMod(None)
                        && (registration.resident.as_deref() == Some(id.as_str())
                            || (registration.inbox_address.is_some() && registration.inbox_address == live.inbox_address))
                });
                if let Some((id, live)) = found {
                    live.route = Route::ClaudeMod(Some(session.clone()));
                    live.about.session = Some(session.clone());
                    if let Err(error) = live.records.write_about(&live.about) {
                        (self.log)(error);
                    }
                    paired.push((id.clone(), session));
                }
            }
        }
        for (id, session) in paired {
            for delivery in self.take_waiting(&id) {
                if let Err(error) = super::claude::deliver(&self.config.world_dir, &session, &mod_delivery(delivery)) {
                    (self.log)(format!("cannot move a waiting delivery into {id}'s session: {error}"));
                }
            }
        }
    }

    /// Reads what the Claude mods wrote: pairs new sessions, turns their events into the
    /// residents' `now`, and records which deliveries reached the model.
    async fn follow_claude(self: Arc<Self>) {
        // The mod looks at its inbox every 2 s; looking back at the same pace keeps `now` at
        // most one look behind what the mod reported.
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            ticker.tick().await;
            self.pair_claude_sessions();
            let sessions: Vec<(String, Records)> = {
                let state = self.state.lock();
                state
                    .residents
                    .values()
                    .filter_map(|live| match &live.route {
                        Route::ClaudeMod(Some(session)) => Some((session.clone(), live.records.clone())),
                        _ => None,
                    })
                    .collect()
            };
            for (session, records) in sessions {
                let harvest = match super::claude::harvest(&self.config.world_dir, &session) {
                    Ok(harvest) => harvest,
                    Err(error) => {
                        (self.log)(error);
                        continue;
                    }
                };
                for event in harvest.events {
                    let state = match event.kind.as_str() {
                        "turn_start" => "busy",
                        "turn_complete" => "idle",
                        "session_end" => "left",
                        _ => continue,
                    };
                    let mut now = records.now().ok().flatten().unwrap_or_default();
                    now.state = state.to_string();
                    now.since = event.at;
                    let _ = records.write_now(&now);
                }
                for delivered in harvest.delivered {
                    let _ = records.append_message(&MessageLine {
                        at: crate::world::now_rfc3339(),
                        kind: "read".into(),
                        peer: delivered,
                        turn: None,
                        text: None,
                    });
                }
            }
        }
    }

    /// Takes everything waiting for a resident, oldest first; the caller has delivered it.
    pub(crate) fn take_waiting(&self, id: &str) -> Vec<Delivery> {
        let dir = {
            let state = self.state.lock();
            match state.residents.get(id) {
                Some(live) => live.records.dir.join("inbox"),
                None => return Vec::new(),
            }
        };
        let mut waiting: Vec<(PathBuf, Delivery)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let delivery = serde_json::from_slice::<Delivery>(&std::fs::read(&path).ok()?).ok()?;
                Some((path, delivery))
            })
            .collect();
        waiting.sort_by(|left, right| left.1.id.cmp(&right.1.id));
        waiting
            .into_iter()
            .map(|(path, delivery)| {
                if let Err(error) = std::fs::remove_file(&path) {
                    (self.log)(format!("cannot remove the delivered {}: {error}", crate::paths::display_path(&path)));
                }
                delivery
            })
            .collect()
    }

    fn enqueue(&self, to: &str, delivery: Delivery) -> Result<(), String> {
        let dir = {
            let state = self.state.lock();
            let live = state.residents.get(to).ok_or_else(|| format!("No resident named {to} lives on {}.", self.config.machine))?;
            live.records.dir.join("inbox")
        };
        std::fs::create_dir_all(&dir).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&dir)))?;
        let bytes = serde_json::to_vec(&delivery).map_err(|error| format!("Cannot encode a delivery: {error}"))?;
        crate::world::write_atomic(&dir.join(format!("{}.json", delivery.id)), &bytes)
    }

    async fn follow_codex(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<HostEvent>) {
        while let Some(event) = events.recv().await {
            self.on_codex(event).await;
        }
    }

    async fn on_codex(&self, event: HostEvent) {
        match event {
            HostEvent::TurnStarted { thread, .. } => {
                if let Some(live) = self.live_for_thread(&thread) {
                    let _ = live.write_now(&Now { state: "busy".into(), since: crate::world::now_rfc3339(), ..Now::default() });
                }
            }
            HostEvent::TurnCompleted { thread, turn, status, error, duration_ms, last_message } => {
                self.answer(&thread, &turn, &status, error, duration_ms, last_message).await;
            }
            HostEvent::Declined { thread, method } => {
                (self.log)(format!("a Codex thread {thread:?} asked {method}; declined"));
            }
            HostEvent::ServerGone { reason } => (self.log)(reason),
            HostEvent::Item { .. } | HostEvent::Delta { .. } | HostEvent::Status { .. } => {}
        }
    }

    /// A colleague's turn ended: its last message goes, verbatim, to everyone whose words went
    /// into the turn.
    async fn answer(&self, thread: &str, turn: &str, status: &str, error: Option<Value>, duration_ms: Option<u64>, last_message: Option<String>) {
        let (id, readers, records) = {
            let mut state = self.state.lock();
            let Some(id) = state.threads.get(thread).cloned() else { return };
            let Some(live) = state.residents.get_mut(&id) else { return };
            let readers: Vec<String> = std::mem::take(&mut live.readers).into_iter().collect();
            live.turn_started = None;
            (id, readers, live.records.clone())
        };
        let ended = crate::world::now_rfc3339();
        let _ = records.write_now(&Now { state: "idle".into(), since: ended.clone(), ..Now::default() });
        let _ = records.append_message(&MessageLine {
            at: ended.clone(),
            kind: "answered".into(),
            peer: readers.join(","),
            turn: Some(turn.to_string()),
            text: last_message.clone(),
        });
        let how = match (status, &error) {
            ("completed", _) => "turn completed".to_string(),
            ("interrupted", _) => "turn interrupted".to_string(),
            (other, Some(error)) => format!("turn {other}: {}", error.get("message").and_then(Value::as_str).unwrap_or("no reason given")),
            (other, None) => format!("turn {other}"),
        };
        let length = duration_ms.map(format_duration).unwrap_or_default();
        let text = format!(
            "=== colleague {id} on {} ({how}{}) ===\n{}\n=== what it did: fastctx w {id} ===",
            self.config.machine,
            if length.is_empty() { String::new() } else { format!(", {length}") },
            last_message.as_deref().unwrap_or("(it ended the turn without a message)"),
        );
        for reader in readers {
            let delivery = match delivery_id() {
                Ok(delivery_id) => Delivery { id: delivery_id, from: id.clone(), kind: "answer".into(), text: text.clone(), as_tool_output: false },
                Err(error) => {
                    (self.log)(error);
                    continue;
                }
            };
            let outcome = self.deliver(&reader, delivery).await;
            let _ = records.append_message(&MessageLine {
                at: crate::world::now_rfc3339(),
                kind: if outcome.is_ok() { "delivered".into() } else { "undelivered".into() },
                peer: reader.clone(),
                turn: Some(turn.to_string()),
                text: outcome.err(),
            });
        }
    }

    fn live_for_thread(&self, thread: &str) -> Option<Records> {
        let state = self.state.lock();
        let id = state.threads.get(thread)?;
        state.residents.get(id).map(|live| live.records.clone())
    }
}

impl std::fmt::Debug for Town {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Town").field("machine", &self.config.machine).finish_non_exhaustive()
    }
}

/// How speaking to a resident landed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Spoken {
    NewTurn,
    JoinedTurn,
    /// Handed to its host's plugin, or waiting for its next World tool response.
    Queued,
}

fn mod_delivery(delivery: Delivery) -> super::claude::ModDelivery {
    super::claude::ModDelivery { id: delivery.id, from: delivery.from, kind: delivery.kind, text: delivery.text, as_user: false }
}

fn subdirectories(dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn delivery_id() -> Result<String, String> {
    let random = crate::world::crypto::random_bytes::<4>()?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    Ok(format!("{nanos:024}-{}", hex::encode(random)))
}

fn format_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// What a colleague is told about where it is and whom it works for.
fn colleague_instructions(id: &str, requester: &str, town: Option<&str>, machine: &str) -> String {
    let place = match town {
        Some(town) => format!("in the town (project) \"{town}\""),
        None => "outside every town".to_string(),
    };
    format!(
        "You are {id}, a resident of the FastCtx World on the machine \"{machine}\", {place}. \
{requester} asked you to work for it. You work for {requester}, not for a human: there is no user \
here to ask. The last message of each of your turns is delivered to {requester} word for word, and \
only to the residents whose words went into that turn; nobody else reads it. End every turn with a \
plain report {requester} can act on: what you did, what you ran to check it and the exit codes, what \
still fails or is left, and a direct answer to every question you were asked. If you need a decision, \
ask it in that last message and stop; the answer comes back as your next turn. Use the World's tools \
for files and commands; your own shell is not available here."
    )
}
