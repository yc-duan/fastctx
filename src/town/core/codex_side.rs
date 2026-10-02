//! Codex residents: threads of this machine's app-server. Starting them, following what the
//! app-server reports, and handing each finished turn's last message to the residents whose
//! words went into that turn.

use super::{
    About, Asked, Birth, Delivery, HashMap, Live, MessageLine, Now, Records, Route, Town, delivery_id, format_duration,
    new_resident_id, resident_instructions, set_desk, set_state,
};
use crate::control::codex_config;
use crate::hands::text;
use crate::town::codex::host::{HostEvent, Input, ThreadSpec};
use crate::town::talk::TalkEvent;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::mpsc;

impl Town {
    /// Starts a Codex colleague for `requester` on this machine, in the requester's town.
    pub(crate) async fn ask_colleague(&self, requester: &str, prompt: &str, label: &str) -> Result<Asked, String> {
        let town = {
            let state = self.state.lock();
            let live = state.residents.get(requester).ok_or_else(|| format!("{requester} is not a resident of this machine."))?;
            live.about.town.clone()
        };
        let birth = Birth::Colleague { requester: requester.to_string(), label: label.to_string() };
        let first = format!("=== from {requester} ===\n{prompt}");
        let id = self.start_codex(birth, town, BTreeSet::from([requester.to_string()]), first, (requester, prompt)).await?;
        Ok(Asked { id, machine: self.config.machine.clone() })
    }

    /// Opens a resident for the user on this machine, in a town or outside every town, and
    /// puts the user's first words in as the user (`specs/world-colleagues.md`, "用户在任何地方
    /// 和居民说话"). Codex is a thread of this machine's app-server; Claude is a background
    /// session started with `claude --bg`.
    pub(crate) async fn open_resident(&self, agent: &str, town: Option<String>, words: &str) -> Result<String, String> {
        if let Some(town) = &town {
            let charter = crate::town::project::charter(&self.config.world_dir, town)?;
            if !charter.machines.iter().any(|machine| machine == &self.config.machine) {
                return Err(format!("{} is not one of {town}'s machines ({}).", self.config.machine, charter.machines.join(", ")));
            }
        }
        match agent {
            "codex" => self.start_codex(Birth::User, town, BTreeSet::new(), words.to_string(), ("user", words)).await,
            "claude" => self.start_claude(town, words).await,
            other => Err(format!("The World opens codex or claude residents; {other} is neither.")),
        }
    }

    /// Creates a Codex resident's records and thread and sends its first words; `readers` get
    /// the last message of that first turn.
    async fn start_codex(
        &self,
        birth: Birth,
        town: Option<String>,
        readers: BTreeSet<String>,
        first: String,
        (asker, asked): (&str, &str),
    ) -> Result<String, String> {
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
            birth,
            since: crate::world::now_rfc3339(),
            session: None,
            town,
        };
        records.write_about(&about)?;
        records.write_now(&Now { state: "busy".into(), since: about.since.clone(), ..Now::default() })?;
        records.append_message(&MessageLine {
            at: about.since.clone(),
            kind: "asked".into(),
            peer: asker.to_string(),
            turn: None,
            text: Some(asked.to_string()),
        })?;
        let spec = self.codex_spec(&about);
        self.state.lock().residents.insert(
            id.clone(),
            Live {
                records: records.clone(),
                about: about.clone(),
                route: Route::Piggyback,
                inbox_address: None,
                sessions: 0,
                readers: HashMap::new(),
            },
        );
        let failed = |error: String| {
            self.state.lock().residents.remove(&id);
            set_state(&records, "error");
            error
        };
        let (thread, model) = self.codex.create_thread(spec).await.map_err(failed)?;
        let mut about = about;
        about.session = Some(thread.clone());
        about.model_source = model.as_ref().map(|_| "host".to_string());
        about.model = model;
        records.write_about(&about)?;
        {
            let mut state = self.state.lock();
            state.threads.insert(thread.clone(), id.clone());
            if let Some(live) = state.residents.get_mut(&id) {
                live.about = about;
                live.route = Route::CodexThread(thread.clone());
            }
        }
        let _turns = self.turns.lock().await;
        let (_, turn) = self.codex.send(&thread, Input::Text(first)).await.map_err(failed)?;
        if let Some(live) = self.state.lock().residents.get_mut(&id) {
            live.readers.insert(turn, readers);
        }
        Ok(id)
    }

    /// What a Codex resident's thread is started and resumed with: its place and whom it works
    /// for, the town's map when it is in one, and the World's tools under its own name.
    pub(super) fn codex_spec(&self, about: &About) -> ThreadSpec {
        let colleague = matches!(about.birth, Birth::Colleague { .. });
        let mut env = json!({
            "FASTCTX_RESIDENT": about.id,
            "HOME": self.config.home,
            "USERPROFILE": self.config.home,
        });
        if colleague {
            env["FASTCTX_COLLEAGUE"] = json!("1");
        }
        ThreadSpec {
            cwd: self.standing_dir(about.town.as_deref()),
            developer_instructions: resident_instructions(about, &self.config.machine, self.map(about.town.as_deref())),
            // The same timeouts `fastctx apply` registers, and host limits opened past the
            // World's own budget so Codex never cuts a result after us (`specs/world-view.md`).
            config: json!({
                "tool_output_token_limit": text::CODEX_OUTPUT_TOKEN_LIMIT,
                "mcp_servers": {"fastctx": {
                    "command": self.config.fastctx,
                    "args": ["serve", "--writer", "apply_patch"],
                    "env": env,
                    "startup_timeout_sec": codex_config::STARTUP_TIMEOUT_SECONDS,
                    "tool_timeout_sec": codex_config::TOOL_TIMEOUT_SECONDS,
                    "tools": {"run": {"output_token_limit": text::CODEX_OUTPUT_TOKEN_LIMIT}},
                }},
            }),
        }
    }

    /// Stops the turn a resident is running, at the user's word. Its thread stays; the next
    /// words start a new turn. Only a Codex resident can be stopped from outside its host.
    pub(crate) async fn interrupt(&self, id: &str) -> Result<bool, String> {
        let route = self.state.lock().residents.get(id).map(|live| live.route.clone());
        match route {
            Some(Route::CodexThread(thread)) => self.codex.interrupt(&thread).await,
            Some(_) => Err(format!("{id}'s host offers the World no way to stop its turn.")),
            None => Err(format!("No resident named {id} lives on {}.", self.config.machine)),
        }
    }

    pub(super) async fn follow_codex(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<HostEvent>) {
        while let Some(event) = events.recv().await {
            self.on_codex(event).await;
        }
    }

    async fn on_codex(&self, event: HostEvent) {
        match event {
            HostEvent::TurnStarted { thread, turn } => {
                if let Some(records) = self.records_for_thread(&thread) {
                    set_state(&records, "busy");
                }
                self.publish_thread(&thread, TalkEvent::TurnStarted { turn });
            }
            HostEvent::TurnCompleted { thread, turn, status, error, duration_ms, last_message } => {
                let readers = self.take_readers(&thread, &turn).await;
                self.publish_thread(&thread, TalkEvent::TurnEnded { turn: turn.clone(), status: status.clone() });
                self.desk_turn_ended(&thread).await;
                self.answer(&thread, &turn, &status, error, duration_ms, last_message, readers).await;
            }
            HostEvent::ItemStarted { thread, item, .. } => {
                if let Some(event) = crate::town::talk::codex_item(&item, false) {
                    self.publish_thread(&thread, event);
                }
            }
            HostEvent::Item { thread, item, .. } => {
                self.desk_item(&thread, &item).await;
                if let Some(event) = crate::town::talk::codex_item(&item, true) {
                    self.publish_thread(&thread, event);
                }
            }
            HostEvent::Delta { thread, item, delta } => self.publish_thread(&thread, TalkEvent::Delta { item, text: delta }),
            HostEvent::Declined { thread, method } => {
                (self.log)(format!("a Codex thread {thread:?} asked {method}; declined"));
            }
            HostEvent::ServerGone { reason } => {
                (self.log)(reason);
                self.server_gone().await;
            }
        }
    }

    fn records_for_thread(&self, thread: &str) -> Option<Records> {
        let state = self.state.lock();
        let id = state.threads.get(thread)?;
        state.residents.get(id).map(|live| live.records.clone())
    }

    /// The residents whose words went into a turn, taken once it ended.
    async fn take_readers(&self, thread: &str, turn: &str) -> Vec<String> {
        let _turns = self.turns.lock().await;
        let mut state = self.state.lock();
        let Some(id) = state.threads.get(thread).cloned() else { return Vec::new() };
        let Some(live) = state.residents.get_mut(&id) else { return Vec::new() };
        live.readers.remove(turn).unwrap_or_default().into_iter().collect()
    }

    /// The app-server stopped, and every turn it was running with it. Their readers hear that
    /// the turn was interrupted, and the Computer Use desk is free.
    async fn server_gone(&self) {
        let dying: Vec<(String, Vec<(String, Vec<String>)>)> = {
            let _turns = self.turns.lock().await;
            let mut state = self.state.lock();
            state
                .residents
                .values_mut()
                .filter_map(|live| {
                    let Route::CodexThread(thread) = &live.route else { return None };
                    let busy = live.records.now().ok().flatten().is_some_and(|now| now.state == "busy");
                    if !busy && live.readers.is_empty() {
                        return None;
                    }
                    let mut turns: Vec<(String, Vec<String>)> =
                        live.readers.drain().map(|(turn, readers)| (turn, readers.into_iter().collect())).collect();
                    if turns.is_empty() {
                        turns.push((String::new(), Vec::new()));
                    }
                    Some((thread.clone(), turns))
                })
                .collect()
        };
        let reason = Some(json!({"message": "its app-server stopped"}));
        for (thread, turns) in dying {
            for (turn, readers) in turns {
                self.publish_thread(&thread, TalkEvent::TurnEnded { turn: turn.clone(), status: "interrupted".into() });
                self.answer(&thread, &turn, "interrupted", reason.clone(), None, None, readers).await;
            }
        }
        let (holder, notices) = {
            let mut state = self.state.lock();
            let (holder, notices) = state.desk.server_gone(&self.config.machine);
            (holder.and_then(|holder| state.residents.get(&holder).map(|live| live.records.clone())), notices)
        };
        if let Some(records) = holder {
            set_desk(&records, false);
        }
        for notice in notices {
            self.desk_notice(notice).await;
        }
    }

    /// A Computer Use call finished: whoever's call went through first sits at the desk; whoever
    /// the helper refused is told who holds it.
    async fn desk_item(&self, thread: &str, item: &Value) {
        let change = {
            let mut state = self.state.lock();
            let Some(resident) = state.threads.get(thread).cloned() else { return };
            match state.desk.item_completed(&resident, item, &self.config.machine) {
                crate::town::desk::Change::Sat => state.residents.get(&resident).map(|live| Err(live.records.clone())),
                crate::town::desk::Change::Notice(notice) => Some(Ok(notice)),
                crate::town::desk::Change::Nothing => None,
            }
        };
        match change {
            Some(Err(records)) => set_desk(&records, true),
            Some(Ok(notice)) => self.desk_notice(notice).await,
            None => {}
        }
    }

    /// A turn ended: if its resident sat at the desk, the desk is free and whoever waits hears.
    async fn desk_turn_ended(&self, thread: &str) {
        let (records, notices) = {
            let mut state = self.state.lock();
            let Some(resident) = state.threads.get(thread).cloned() else { return };
            let (held, notices) = state.desk.turn_ended(&resident, &self.config.machine);
            (held.then(|| state.residents.get(&resident).map(|live| live.records.clone())).flatten(), notices)
        };
        if let Some(records) = records {
            set_desk(&records, false);
        }
        for notice in notices {
            self.desk_notice(notice).await;
        }
    }

    async fn desk_notice(&self, notice: crate::town::desk::Notice) {
        let delivery = match delivery_id() {
            Ok(id) => Delivery { id, from: "world".into(), kind: "notice".into(), text: notice.text, as_tool_output: false },
            Err(error) => return (self.log)(error),
        };
        if let Err(error) = self.deliver(&notice.to, delivery).await {
            (self.log)(format!("cannot tell {} about the Computer Use desk: {error}", notice.to));
        }
    }

    /// A Codex resident's turn ended: its last message goes, verbatim, to everyone whose words
    /// went into the turn.
    #[allow(clippy::too_many_arguments)]
    async fn answer(
        &self,
        thread: &str,
        turn: &str,
        status: &str,
        error: Option<Value>,
        duration_ms: Option<u64>,
        last_message: Option<String>,
        readers: Vec<String>,
    ) {
        let (id, records, requester_here) = {
            let state = self.state.lock();
            let Some(id) = state.threads.get(thread).cloned() else { return };
            let Some(live) = state.residents.get(&id) else { return };
            let requester_here = match &live.about.birth {
                Birth::Colleague { requester, .. } => self.present(&state, requester),
                Birth::User => true,
            };
            (id, live.records.clone(), requester_here)
        };
        let ended = crate::world::now_rfc3339();
        {
            let mut now = records.now().ok().flatten().unwrap_or_default();
            now.state = if requester_here { "idle" } else { "asleep" }.to_string();
            now.since = ended.clone();
            if !readers.is_empty() {
                now.last_action = Some(format!("answered {}", readers.join(", ")));
                now.last_action_at = Some(ended.clone());
            }
            let _ = records.write_now(&now);
        }
        let _ = records.append_message(&MessageLine {
            at: ended.clone(),
            kind: "answered".into(),
            peer: readers.join(","),
            turn: Some(turn.to_string()),
            text: last_message.clone(),
        });
        let reason = error.as_ref().and_then(|error| error.get("message")).and_then(Value::as_str);
        let how = match (status, reason) {
            ("completed", _) => "turn completed".to_string(),
            (other, Some(reason)) => format!("turn {other}: {reason}"),
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
}
