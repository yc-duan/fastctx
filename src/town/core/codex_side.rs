//! Codex residents: threads of this machine's app-server. Starting them, and turning what the
//! app-server reports into the course of their turns (`turns`).

use super::turns::{Report, TurnEnd};
use super::{About, Asked, Birth, Delivery, Live, MessageLine, Now, Records, Route, Town, delivery_id, new_resident_id, resident_instructions};
use crate::control::codex_config;
use crate::hands::text;
use crate::town::codex::host::{HostEvent, Input, Landed, ThreadSpec};
use crate::town::remote::{Letter, Peer};
use crate::town::talk::TalkEvent;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::mpsc;

impl Town {
    /// Starts a Codex colleague on this machine for `requester`, who lives here or on another
    /// machine, in the requester's town, which must reach this machine.
    pub(crate) async fn ask_colleague(&self, requester: &Peer, town: Option<String>, prompt: &str, label: &str) -> Result<Asked, String> {
        if let Some(town) = &town {
            let charter = crate::town::project::charter(&self.config.world_dir, town)?;
            if !charter.machines.iter().any(|machine| machine == &self.config.machine) {
                return Err(format!(
                    "{} is not one of {town}'s machines ({}); the user can add it to the project first.",
                    self.config.machine,
                    charter.machines.join(", ")
                ));
            }
        }
        let elsewhere = requester.machine != self.config.machine;
        let birth = Birth::Colleague {
            requester: requester.id.clone(),
            label: label.to_string(),
            machine: elsewhere.then(|| requester.machine.clone()),
        };
        let name = self.peer_name(requester);
        let first = format!("=== from {name} ===\n{prompt}");
        let id = self.start_codex(birth, town, BTreeSet::from([requester.clone()]), first, (&name, prompt)).await?;
        Ok(Asked { id, machine: self.config.machine.clone() })
    }

    /// The town a resident of this machine is in, for a colleague it asks for.
    pub(crate) fn requester_town(&self, id: &str) -> Result<Option<String>, String> {
        self.town_of(id).ok_or_else(|| format!("{id} is not a resident of this machine."))
    }

    /// A resident of another machine that asked for colleagues here has left, as its machine
    /// wrote: those not in a turn sleep, and the others sleep when their turn ends, until it
    /// speaks again.
    pub(crate) fn requester_left(&self, requester: Peer) {
        let mut state = self.state.lock();
        for colleague in state.residents.values() {
            if let (Birth::Colleague { requester: id, machine: Some(machine), .. }, Route::CodexThread(thread)) =
                (&colleague.about.birth, &colleague.route)
                && *id == requester.id
                && *machine == requester.machine
                && !self.codex.is_running(thread)
            {
                self.set_state(&colleague.records, "asleep");
            }
        }
        state.departed.insert(requester);
    }

    /// A resident of this machine left: every other machine where it asked for colleagues
    /// hears, so they sleep there as they do here.
    pub(super) fn tell_colleagues_left(&self, id: &str, records: &Records) {
        let machines: BTreeSet<String> = match records.colleagues_asked() {
            Ok(asked) => asked.into_iter().map(|(machine, _)| machine).filter(|machine| *machine != self.config.machine).collect(),
            Err(error) => return (self.log)(format!("cannot tell where {id} asked for colleagues: {error}")),
        };
        for machine in machines {
            if let Err(error) = self.mail(&machine, &Letter::Left { requester: id.to_string() }) {
                (self.log)(format!("cannot tell {machine} that {id} left: {error}"));
            }
        }
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
        readers: BTreeSet<Peer>,
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
        self.state.lock().residents.insert(id.clone(), Live::new(records.clone(), about.clone(), Route::Piggyback, None, 0));
        let failed = |error: String| {
            self.state.lock().residents.remove(&id);
            self.set_state(&records, "error");
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
        self.codex_words(&id, &thread, Input::Text(first), readers).await.map_err(failed)?;
        Ok(id)
    }

    /// Puts words into a Codex resident's thread; everyone in `readers` is owed the end of the
    /// turn they went into. Returns how they landed and that turn. A thread whose fastctx tools
    /// are missing works without the World's hands, which its watchers and everyone owed the
    /// turn's end hear at once.
    pub(super) async fn codex_words(&self, id: &str, thread: &str, input: Input, readers: impl IntoIterator<Item = Peer>) -> Result<(Landed, String), String> {
        let landed = self.codex_send(id, thread, input, readers).await?;
        if let Some(text) = self.codex.tools_missing(thread) {
            self.health(id, Report { text, tell: Some("tools") }).await;
        }
        Ok(landed)
    }

    /// [`Self::codex_words`] without telling anyone that the thread's tools are missing.
    pub(super) async fn codex_send(&self, id: &str, thread: &str, input: Input, readers: impl IntoIterator<Item = Peer>) -> Result<(Landed, String), String> {
        // The thread's tools start before words go in, outside the lock every other resident's
        // words wait on.
        self.codex.prepare(thread).await?;
        let (landed, turn) = {
            // Held until the turn's readers are recorded: its end takes them under this lock.
            let _turns = self.turns.lock().await;
            let (landed, turn) = self.codex.send(thread, input).await?;
            if let Some(records) = self.owe(id, &turn, readers)
                && landed == Landed::NewTurn
            {
                self.set_state(&records, "busy");
            }
            (landed, turn)
        };
        Ok((landed, turn))
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
                if let Some(records) = self.resident_of(&thread).and_then(|id| self.turn_started(&id, &turn)) {
                    self.set_state(&records, "busy");
                }
                self.publish_thread(&thread, TalkEvent::TurnStarted { turn });
            }
            HostEvent::TurnCompleted { thread, turn, status, error, duration_ms, last_message } => {
                self.desk_turn_ended(&thread).await;
                let Some(id) = self.resident_of(&thread) else { return };
                let readers = self.take_readers(&id, &turn).await;
                let reason = error.as_ref().and_then(|error| error.get("message")).and_then(Value::as_str).map(str::to_string);
                self.turn_ended(&id, TurnEnd { turn: Some(turn), status, reason, duration_ms, last_message, readers, outage: false }).await;
            }
            HostEvent::ItemStarted { thread, item, .. } => {
                {
                    let mut state = self.state.lock();
                    if let Some(resident) = state.threads.get(&thread).cloned() {
                        state.desk.item_started(&resident, &item);
                    }
                }
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
            HostEvent::Health { thread, text, urgent } => {
                if let Some(id) = self.resident_of(&thread) {
                    self.health(&id, Report { text, tell: urgent }).await;
                }
            }
            HostEvent::Declined { thread, method } => {
                (self.log)(format!("a Codex thread {thread:?} asked {method}; declined"));
            }
            HostEvent::ServerGone { reason, turns } => {
                (self.log)(reason);
                self.server_gone(turns).await;
            }
        }
    }

    /// The resident a Codex thread is.
    fn resident_of(&self, thread: &str) -> Option<String> {
        self.state.lock().threads.get(thread).cloned()
    }

    /// The app-server stopped, and every turn it was running with it: each ends as interrupted,
    /// told to everyone owed its end, and the Computer Use desk is free. Turns words started
    /// since, in the app-server that replaced it, run on.
    async fn server_gone(&self, turns: Vec<(String, String)>) {
        for (thread, turn) in turns {
            let Some(id) = self.resident_of(&thread) else { continue };
            let readers = self.take_readers(&id, &turn).await;
            let reason = Some("its app-server stopped".to_string());
            self.turn_ended(&id, TurnEnd { turn: Some(turn), status: "interrupted".into(), reason, duration_ms: None, last_message: None, readers, outage: true })
                .await;
        }
        let (holder, notices) = {
            let mut state = self.state.lock();
            let (holder, notices) = state.desk.server_gone(&self.config.machine);
            (holder.and_then(|holder| state.residents.get(&holder).map(|live| live.records.clone())), notices)
        };
        if let Some(records) = holder {
            self.set_desk(&records, false);
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
                crate::town::desk::Change::Notices(notices) => Some(Ok(notices)),
                crate::town::desk::Change::Nothing => None,
            }
        };
        match change {
            Some(Err(records)) => self.set_desk(&records, true),
            Some(Ok(notices)) => {
                for notice in notices {
                    self.desk_notice(notice).await;
                }
            }
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
            self.set_desk(&records, false);
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
}
