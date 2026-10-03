//! The town core on one machine: who lives here, what they do, and the words on their way to
//! them.
//!
//! It is driven from three sides: the control center registers every MCP session as a resident
//! and reports its hands' actions; the resident tools ask for colleagues and speak to them; the
//! hosts report what their sessions do. This file keeps the registry; the hosts' sides, the
//! delivery of words and the live watching are in the submodules.

mod claude_side;
mod codex_side;
mod delivery;
mod watch;

use super::codex::host::{CodexHost, HostEvent};
use super::records::{About, Birth, MessageLine, Now, Records, new_resident_id};
use super::talk::TalkEvent;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// with its mod's registration and recognizes the session when it reconnects.
    inbox_address: Option<String>,
    /// MCP sessions currently attached for this resident.
    sessions: u32,
    /// For each Codex turn, the residents whose words went into it; they get its last message.
    readers: HashMap<String, BTreeSet<String>>,
    /// The Codex turn running now, as the host reported or answered it. Kept here rather than
    /// read from `now`, whose write can fail while someone reads it.
    turn: Option<String>,
}

#[derive(Default)]
struct State {
    residents: HashMap<String, Live>,
    /// Codex thread id → resident id.
    threads: HashMap<String, String>,
    /// Who is watching each resident live.
    watchers: HashMap<String, Vec<mpsc::UnboundedSender<TalkEvent>>>,
    /// This machine's Computer Use desk.
    desk: super::desk::Desk,
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
    /// The home the node runs in. A resident's MCP server is told it explicitly: the Codex
    /// app-server gives the servers it starts the operating system's idea of the user's home,
    /// and an MCP server that finds a different home reaches a different control center.
    pub(crate) home: PathBuf,
}

pub(crate) struct Town {
    config: TownConfig,
    codex: Arc<CodexHost>,
    log: Arc<dyn Fn(String) + Send + Sync>,
    state: Mutex<State>,
    /// Held while words go into a Codex turn and while a finished turn's readers are taken,
    /// so an answer goes to exactly the residents whose words went into that turn.
    turns: tokio::sync::Mutex<()>,
    /// Held while a waiting queue is read, added to or moved, so no words are taken twice.
    inbox: Mutex<()>,
}

/// What `ask_colleague` hands back.
pub(crate) struct Asked {
    pub(crate) id: String,
    pub(crate) machine: String,
}

/// How speaking to a resident landed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Spoken {
    NewTurn,
    JoinedTurn,
    /// Handed to its host's plugin, or waiting for its next World tool response.
    Queued,
}

impl Town {
    pub(crate) fn new(
        config: TownConfig,
        codex: Arc<CodexHost>,
        events: mpsc::UnboundedReceiver<HostEvent>,
        log: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Arc<Self> {
        let town = Arc::new(Self {
            config,
            codex,
            log,
            state: Mutex::new(State::default()),
            turns: tokio::sync::Mutex::new(()),
            inbox: Mutex::new(()),
        });
        town.restore();
        tokio::spawn(Arc::clone(&town).follow_codex(events));
        tokio::spawn(Arc::clone(&town).follow_claude());
        tokio::spawn(Arc::clone(&town).reconcile_jobs());
        town
    }

    pub(crate) fn log(&self, line: String) {
        (self.log)(line);
    }

    pub(crate) fn machine(&self) -> &str {
        &self.config.machine
    }

    /// `~/.fastctx/world` on this machine.
    pub(crate) fn world_dir(&self) -> &Path {
        &self.config.world_dir
    }

    /// The home this node runs in, where `run` starts when no cwd is given.
    pub(crate) fn home(&self) -> &Path {
        &self.config.home
    }

    /// The town a resident on this machine is in: `Some(None)` outside every town, `None` when
    /// no such resident lives here.
    pub(crate) fn town_of(&self, id: &str) -> Option<Option<String>> {
        self.state.lock().residents.get(id).map(|live| live.about.town.clone())
    }

    /// The directory holding a resident's records.
    pub(crate) fn resident_dir(&self, town: Option<&str>, id: &str) -> PathBuf {
        match town {
            Some(town) => self.config.world_dir.join("towns").join(town).join("residents").join(id),
            None => self.config.world_dir.join("outside").join("residents").join(id),
        }
    }

    /// A town's bookkeeping directory: where residents the World launches stand.
    pub(crate) fn town_dir(&self, town: &str) -> PathBuf {
        self.config.world_dir.join("towns").join(town)
    }

    /// Where a resident the World starts stands: its town's directory, or the outside.
    fn standing_dir(&self, town: Option<&str>) -> PathBuf {
        match town {
            Some(town) => self.town_dir(town),
            None => self.config.world_dir.join("outside"),
        }
    }

    /// Every directory that holds records of residents on this machine, in towns and outside.
    fn record_dirs(&self) -> Vec<PathBuf> {
        let parents = std::iter::once(self.config.world_dir.join("outside").join("residents"))
            .chain(subdirectories(&self.config.world_dir.join("towns")).into_iter().map(|town| town.join("residents")));
        parents.flat_map(|parent| subdirectories(&parent)).collect()
    }

    /// Every directory holding records of one resident on this machine.
    pub(crate) fn resident_dirs(&self, id: &str) -> Vec<PathBuf> {
        self.record_dirs().into_iter().filter(|dir| dir.file_name().is_some_and(|name| name == id)).collect()
    }

    /// Whether any resident ever recorded on this machine had this id, in or outside a town.
    fn recorded(&self, id: &str) -> bool {
        self.resident_dir(None, id).exists()
            || subdirectories(&self.config.world_dir.join("towns")).iter().any(|town| town.join("residents").join(id).exists())
    }

    /// The records a resident lives in now, of those it left in several places as it moved
    /// between towns: the ones it has not left, else the latest.
    fn current_records(&self, id: &str) -> Option<(Records, About)> {
        self.record_dirs()
            .into_iter()
            .filter(|dir| dir.file_name().is_some_and(|name| name == id))
            .filter_map(|dir| {
                let records = Records::new(dir);
                let about = records.about().ok().flatten()?;
                let now = records.now().ok().flatten().unwrap_or_default();
                Some((super::records::currency(&now), records, about))
            })
            .max_by_key(|(currency, ..)| *currency)
            .map(|(_, records, about)| (records, about))
    }

    /// Registers an MCP session as a resident. A session the World launched names itself in
    /// `FASTCTX_RESIDENT`; a Claude session reconnecting with the inbox address it had is the
    /// same resident again; any other session becomes a new resident born of the user.
    pub(crate) fn attach(
        &self,
        agent: &str,
        agent_version: Option<&str>,
        named: Option<&str>,
        surface: &str,
        inbox_address: Option<&str>,
    ) -> Result<String, String> {
        let mut state = self.state.lock();
        let known = match named {
            Some(id) => Some(id.to_string()),
            None => inbox_address.and_then(|address| {
                state
                    .residents
                    .iter()
                    .find(|(_, live)| live.inbox_address.as_deref() == Some(address))
                    .map(|(id, _)| id.clone())
                    .or_else(|| self.resident_by_inbox(address))
            }),
        };
        if let Some(id) = &known {
            if !state.residents.contains_key(id)
                && let Some((records, about)) = self.current_records(id)
            {
                let route = route_for(&about.agent);
                let inbox_address = inbox_address.map(str::to_string);
                state.residents.insert(id.clone(), Live { records, about, route, inbox_address, sessions: 0, readers: HashMap::new(), turn: None });
            }
            if let Some(live) = state.residents.get_mut(id) {
                live.sessions += 1;
                if live.records.now().ok().flatten().is_some_and(|now| now.state == "left") {
                    self.set_state(&live.records, "unknown");
                }
                return Ok(id.clone());
            }
        }
        let id = match known {
            Some(id) => id,
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
        let route = route_for(agent);
        state.residents.insert(
            id.clone(),
            Live { records, about, route, inbox_address: inbox_address.map(str::to_string), sessions: 1, readers: HashMap::new(), turn: None },
        );
        drop(state);
        self.pair_claude_sessions();
        Ok(id)
    }

    /// The resident a Claude session was before this node started: its mod's registration
    /// carries the session's inbox address, and the resident's records name the session.
    fn resident_by_inbox(&self, address: &str) -> Option<String> {
        let world_dir = &self.config.world_dir;
        let session = super::claude::sessions(world_dir).into_iter().find(|session| {
            super::claude::registration(world_dir, session)
                .ok()
                .flatten()
                .is_some_and(|registration| registration.inbox_address.as_deref() == Some(address))
        })?;
        self.record_dirs()
            .into_iter()
            .filter_map(|dir| Records::new(dir).about().ok().flatten())
            .find(|about| about.session.as_deref() == Some(session.as_str()) && about.machine == self.config.machine)
            .map(|about| about.id)
    }

    /// An MCP session ended. A resident with no session left has left, unless the World holds
    /// its thread, in which case it is only asleep.
    pub(crate) fn detach(&self, id: &str) {
        let mut state = self.state.lock();
        let Some(live) = state.residents.get_mut(id) else { return };
        live.sessions = live.sessions.saturating_sub(1);
        if live.sessions > 0 || matches!(live.route, Route::CodexThread(_)) {
            return;
        }
        self.set_state(&live.records, "left");
        // A colleague works for whoever asked for it; once that resident has left, an idle
        // colleague sleeps until someone speaks to it again.
        for colleague in state.residents.values() {
            if let (Birth::Colleague { requester, .. }, Route::CodexThread(thread)) = (&colleague.about.birth, &colleague.route)
                && requester == id
                && !self.codex.is_running(thread)
            {
                self.set_state(&colleague.records, "asleep");
            }
        }
    }

    /// Whether a resident is here: an MCP session is attached, or the World holds it running.
    fn present(&self, state: &State, id: &str) -> bool {
        state.residents.get(id).is_some_and(|live| {
            live.sessions > 0 || matches!(&live.route, Route::CodexThread(thread) if self.codex.is_running(thread))
        })
    }

    /// Moves a resident on this machine into a town (`fastctx project enter`) and returns the
    /// town's map. What it did so far stays where it happened; from now on its records are in
    /// the town's directory, and words still waiting for it move along.
    pub(crate) fn enter(&self, id: &str, town: &str) -> Result<String, String> {
        let charter = super::project::charter(&self.config.world_dir, town)?;
        if !charter.machines.iter().any(|machine| machine == &self.config.machine) {
            return Err(format!(
                "{} is not one of {town}'s machines ({}); the user can add it to the project first.",
                self.config.machine,
                charter.machines.join(", ")
            ));
        }
        {
            let _inbox = self.inbox.lock();
            let mut state = self.state.lock();
            let live = state.residents.get_mut(id).ok_or_else(|| format!("{id} is not a resident of this machine."))?;
            if live.about.town.as_deref() != Some(town) {
                let records = Records::new(self.resident_dir(Some(town), id));
                records.create()?;
                let mut about = live.about.clone();
                about.town = Some(town.to_string());
                // The words move first, all or none; if the new records cannot be written they
                // move back, and the resident stays where it was.
                delivery::move_waiting(&live.records.dir, &records.dir)?;
                let entered = super::records::now_precise();
                let written = live.records.now().and_then(|now| {
                    records.write_about(&about)?;
                    // Its state is as new as its arrival, so these records outrank the ones it
                    // leaves even if those cannot be marked as left.
                    records.write_now(&Now { since: entered.clone(), ..now.unwrap_or_default() })
                });
                if let Err(error) = written {
                    return Err(match delivery::move_waiting(&records.dir, &live.records.dir) {
                        Ok(()) => error,
                        Err(back) => format!("{error}; {back}"),
                    });
                }
                let left = live.records.update_now(|now| {
                    now.state = "left".into();
                    now.since = entered.clone();
                    now.last_action = Some(format!("entered the town {town}"));
                    now.last_action_at = Some(entered.clone());
                });
                if let Err(error) = left {
                    (self.log)(format!("cannot mark {id} as gone from its previous records: {error}"));
                }
                live.records = records;
                live.about = about;
            }
        }
        super::project::render_map(&self.config.world_dir, town, &self.config.machine)
    }

    /// Records one action of a resident's hands and makes it the last thing it did.
    pub(crate) fn record_action(&self, id: &str, machine: &str, hand: &str, what: &str, result: &str) {
        let state = self.state.lock();
        let Some(live) = state.residents.get(id) else { return };
        let at = crate::world::now_rfc3339();
        match live.records.append_action(&at, machine, hand, what, result) {
            Ok(_) => {
                let changed = live.records.update_now(|now| {
                    now.last_action = Some(format!("{hand} {what}"));
                    now.last_action_at = Some(at);
                });
                if let Err(error) = changed {
                    (self.log)(format!("cannot record the last action of {id}: {error}"));
                }
            }
            Err(error) => (self.log)(format!("cannot record an action of {id}: {error}")),
        }
    }

    /// The town's map, compiled now; `None` outside every town or when it cannot be read.
    fn map(&self, town: Option<&str>) -> Option<String> {
        let town = town?;
        match super::project::render_map(&self.config.world_dir, town, &self.config.machine) {
            Ok(map) => Some(map),
            Err(error) => {
                (self.log)(format!("cannot compile the map of {town}: {error}"));
                None
            }
        }
    }

    /// Takes back what this machine's town held before the node restarted. A Codex resident
    /// the World holds sleeps in its thread and wakes at the next words; a turn it was running
    /// died with the old app-server. Every other resident's session ended with the old node.
    /// A resident that moved between towns left records in each; only the current ones count.
    fn restore(&self) {
        let mut current: BTreeMap<String, ((bool, Option<time::OffsetDateTime>), Records, About, Now)> = BTreeMap::new();
        for dir in self.record_dirs() {
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
            let rank = super::records::currency(&now);
            let better = current.get(&about.id).is_none_or(|(held, ..)| rank > *held);
            if better {
                current.insert(about.id.clone(), (rank, records, about, now));
            }
        }
        let mut state = self.state.lock();
        for (id, (_, records, about, now)) in current {
            let thread = match (about.agent.as_str(), about.surface.as_str(), &about.session) {
                ("codex", "world", Some(thread)) => Some(thread.clone()),
                _ => None,
            };
            let state_now = match (&thread, now.state.as_str()) {
                (Some(_), "busy") => Some("interrupted"),
                (Some(_), "interrupted" | "asleep" | "error") => None,
                (Some(_), _) => Some("asleep"),
                (None, "left") => None,
                (None, _) => Some("left"),
            };
            if let Some(word) = state_now {
                self.set_state(&records, word);
            }
            // Nobody sits at the desk of a node that just started.
            if now.desk {
                self.set_desk(&records, false);
            }
            if state_now == Some("interrupted") {
                let _ = records.append_message(&MessageLine {
                    at: crate::world::now_rfc3339(),
                    kind: "interrupted".into(),
                    peer: String::new(),
                    turn: None,
                    text: Some("The node restarted while this turn ran; the turn did not finish.".into()),
                });
            }
            let Some(thread) = thread else { continue };
            self.codex.adopt(&thread, self.codex_spec(&about));
            state.threads.insert(thread.clone(), id.clone());
            state.residents.insert(
                id,
                Live { records, about, route: Route::CodexThread(thread), inbox_address: None, sessions: 0, readers: HashMap::new(), turn: None },
            );
        }
    }
}

impl std::fmt::Debug for Town {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Town").field("machine", &self.config.machine).finish_non_exhaustive()
    }
}

/// How a resident of this agent program is reached before anything else is known about it.
fn route_for(agent: &str) -> Route {
    if agent.to_ascii_lowercase().contains("claude") { Route::ClaudeMod(None) } else { Route::Piggyback }
}

/// A delivery in the shape a Claude session's mod reads.
fn mod_delivery(delivery: Delivery) -> super::claude::ModDelivery {
    super::claude::ModDelivery { id: delivery.id, from: delivery.from, kind: delivery.kind, text: delivery.text, as_user: false }
}

impl Town {
    /// Records whether a resident sits at this machine's Computer Use desk.
    fn set_desk(&self, records: &Records, desk: bool) {
        if let Err(error) = records.update_now(|now| now.desk = desk) {
            (self.log)(format!("cannot record the desk in {}: {error}", crate::paths::display_path(&records.dir)));
        }
    }

    /// Changes a resident's state and keeps what else `now` knows.
    fn set_state(&self, records: &Records, state: &str) {
        let changed = records.update_now(|now| {
            now.state = state.to_string();
            now.since = crate::world::now_rfc3339();
        });
        if let Err(error) = changed {
            (self.log)(format!("cannot record the state {state} in {}: {error}", crate::paths::display_path(&records.dir)));
        }
    }
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect()
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

/// What a resident the World started is told: where it is, whom it works for, and the map of
/// its town. A colleague works for the resident that asked for it and answers it at the end of
/// every turn; a resident the user opened talks with the user.
fn resident_instructions(about: &About, machine: &str, map: Option<String>) -> String {
    let id = &about.id;
    let place = match &about.town {
        Some(town) => format!("in the town (project) \"{town}\""),
        None => "outside every town".to_string(),
    };
    let mut text = format!("You are {id}, a resident of the FastCtx World on the machine \"{machine}\", {place}. ");
    match &about.birth {
        Birth::Colleague { requester, .. } => text.push_str(&format!(
            "{requester} asked you to work for it. You work for {requester}, not for a human: there is no user \
here to ask. The last message of each of your turns is delivered to {requester} word for word, and \
only to the residents whose words went into that turn; nobody else reads it. End every turn with a \
plain report {requester} can act on: what you did, what you ran to check it and the exit codes, what \
still fails or is left, and a direct answer to every question you were asked. If you need a decision, \
ask it in that last message and stop; the answer comes back as your next turn. "
        )),
        Birth::User => text.push_str(
            "The user opened you through the World and talks with you from their own devices; the user's \
words reach you as user messages, and words from other residents arrive marked with their name. ",
        ),
    }
    text.push_str("Use the World's tools for files and commands; your own shell is not available here.");
    if let Some(map) = map {
        text.push_str("\n\n");
        text.push_str(&map);
    }
    text
}
