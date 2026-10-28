//! Who lives on this machine: finding a session's resident or making one,
//! knowing who is here, and, for a session whose host tells the World when
//! it works, what it is doing and who is owed the end of its turn.
//!
//! A resident the World started is named in its environment. A Codex
//! session is recognised by its thread, which stays the same across MCP
//! connections. A Claude session is registered by its mod; its process
//! lifetime establishes presence. Other sessions are residents from their
//! first call, present while their connection stays open. One the World
//! started is here unless it is asleep.
//!
//! Who is owed the end of each running turn is kept in memory and written
//! through to its `now` on every change, so a node that restarts knows whom
//! to tell.
//!
//! A resident has one directory, its conversation's home; which project it
//! is in is one record there (`town`), so changing projects moves nothing.

pub(crate) mod cache;
mod departures;
mod records;
mod removal;
mod tombstones;
pub use removal::removal;
pub use tombstones::Tombstone;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use fastctx_types::focus::Focus;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{About, Activity, Hints, Host, Now, Origin, Places, Trouble, Whereabouts};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

pub use records::{
    append_message, last_action, last_action_before, read_about, read_now, read_places, read_removed, record_action,
    record_owned_action, record_seen,
};

pub struct Residents {
    state: Mutex<State>,
    /// Moves whenever a resident may have changed: came, went, or changed
    /// in what is kept of it (`changes`).
    changed: tokio::sync::watch::Sender<u64>,
    /// This machine's name now, which every resident living here is told as
    /// living on, whatever the machine was called when it was born.
    here: std::sync::RwLock<Option<fastctx_types::MachineName>>,
    speaking: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<()>>,
    paying: crate::leases::Registry<ResidentId, fastctx_platform::doing::Mutex<()>>,
    management: crate::leases::Registry<ResidentId, fastctx_platform::doing::RwLock<()>>,
}

struct State {
    live: cache::Cache,
    /// Codex thread id to the resident it is.
    threads: cache::Index,
    current_sessions: cache::Index,
    /// Remote requesters' departures and returns, kept outside the cache.
    departed: departures::Departures,
    inboxes: cache::Index,
    /// The colleagues each asker started, kept as residents are admitted:
    /// a resident's origin never changes, and finding them by walking every
    /// resident on record read the whole history back from disk each time
    /// a session left.
    colleagues: HashMap<Address, Vec<ResidentId>>,
    /// Residents being deleted, with how many deletions of each are under
    /// way: they take nothing new while their turn and commands stop.
    deleting: HashMap<ResidentId, usize>,
    tombstones: tombstones::Tombstones,
}

impl State {
    /// Puts a resident in the cache and, when it is a colleague, under the
    /// one who asked for it.
    fn admit(&mut self, live: Live) {
        let id = live.about.id.clone();
        if let Origin::Colleague { asked_by, .. } = &live.about.origin {
            let started = self.colleagues.entry(asked_by.clone()).or_default();
            if !started.contains(&id) {
                started.push(id.clone());
            }
        }
        self.live.insert(id, live);
    }

    fn record_session(&self, before: Option<&str>, after: &str) {
        if before == Some(after) {
            return;
        }
        if let Some(before) = before {
            let count = self.current_sessions.get::<u64>(before).expect("recorded session");
            if count == 1 {
                self.current_sessions.remove(&before);
            } else {
                self.current_sessions.insert(before, count - 1);
            }
        }
        let count = self.current_sessions.get::<u64>(after).unwrap_or(0);
        self.current_sessions.insert(after, count + 1);
    }

    /// A resident that held `session` is no longer on record.
    fn forget_session(&self, session: &str) {
        match self.current_sessions.get::<u64>(session) {
            Some(count) if count > 1 => self.current_sessions.insert(session, count - 1),
            Some(_) => self.current_sessions.remove(&session),
            None => {}
        }
    }
}

struct StateGuard<'a>(std::sync::MutexGuard<'a, State>, &'a tokio::sync::watch::Sender<u64>);

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
        if !std::thread::panicking()
            && let Err(error) = self.0.live.release()
        {
            cache::failed("spilling inactive resident state", error);
        }
        if self.0.live.take_touched() {
            self.1.send_modify(|changes| *changes = changes.wrapping_add(1));
        }
    }
}

/// A town a resident lived in and moved away from.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Former {
    pub id: ResidentId,
    pub town: Option<TownName>,
    pub to: Option<TownName>,
    pub at: Timestamp,
}

/// What a resident has been told of the town it lives in: the town, and
/// how far into its ledger.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Heard {
    #[serde(default)]
    pub scratch: Option<crate::scratch::Receipt>,
    #[serde(default)]
    pub context: Option<crate::context::Receipt>,
    #[serde(default)]
    pub user_prompt: Option<String>,
    pub town: Option<TownName>,
    pub ledger: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Live {
    removed: Option<Timestamp>,
    about: About,
    places: Places,
    /// A Claude session's inbox address, for commands from its native shell.
    inbox: Option<String>,
    /// Open connections the resident arrived on.
    connections: usize,
    /// Observed host runtime, independent of the conversation's availability.
    #[serde(default, alias = "claude_here")]
    host_present: bool,
    now: Option<Now>,
    /// For each turn running, whose words went into it.
    owed: HashMap<String, BTreeSet<Address>>,
    /// Held while words go into its session until the turn they joined is
    /// known and owed, and while a turn's end is taken: so a turn's end goes
    /// to exactly those whose words went into it.
    /// `None` until it is told who and where it is.
    heard: Option<Heard>,
    /// What the instructions its session started with tell it of its town,
    /// which is all it knows of it after compacting; `None` for a session
    /// that started with none of the World's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instructed: Option<Heard>,
    /// When its host last told of its context compacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compacted_at: Option<Timestamp>,
}

impl Live {
    fn new(about: About, places: Places, now: Option<Now>) -> Self {
        let owed = now
            .iter()
            .flat_map(|now| now.owed.iter())
            .map(|(turn, readers)| (turn.clone(), readers.iter().cloned().collect()))
            .collect();
        Self {
            removed: None,
            about,
            places,
            inbox: None,
            connections: 0,
            host_present: false,
            now,
            owed,
            heard: None,
            instructed: None,
            compacted_at: None,
        }
    }

    fn town(&self) -> Option<TownName> {
        self.places.now().cloned()
    }
}

/// One resident on record, as `fastctx w` needs it.
pub struct OnRecord {
    pub removed: Option<Timestamp>,
    pub id: ResidentId,
    pub town: Option<TownName>,
    pub here: bool,
    pub about: About,
    pub now: Option<Now>,
}

impl Residents {
    pub(crate) fn host_open(&self, id: &ResidentId) -> bool {
        self.lock().live.get(id).is_some_and(|live| live.host_present || live.connections > 0)
    }
    /// Reads every resident this machine has a record of. A record that
    /// cannot be read is reported and left as it is.
    pub fn load(world: &WorldDir) -> io::Result<(Self, Vec<(PathBuf, io::Error)>)> {
        let departed = departures::Departures::load(world)?;
        let root = cache::prepare(world.path())?;
        let mut problems = Vec::new();
        let mut state = State {
            live: cache::Cache::new(root.join("residents"))?,
            threads: cache::Index::new(root.join("threads"))?,
            current_sessions: cache::Index::new(root.join("current-sessions"))?,
            departed,
            inboxes: cache::Index::new(root.join("inboxes"))?,
            colleagues: HashMap::new(),
            deleting: HashMap::new(),
            tombstones: tombstones::Tombstones::load(world, &mut problems)?,
        };
        for kept in world.kept()? {
            let fastctx_types::layout::Kind::Home(id) = &kept.kind else { continue };
            // What a deletion stopped halfway left of it; the node finishes
            // that deletion (`data::delete::finish_interrupted`).
            if state.tombstones.of(id).is_some() {
                continue;
            }
            let directory = &kept.directory;
            // Files attached for a conversation not opened yet have a home
            // and no resident.
            if !directory.about().try_exists()? {
                continue;
            }
            let about = match records::read_about(directory) {
                Ok(about) if about.id == *id => about,
                Ok(about) => {
                    problems.push((directory.about(), io::Error::other(format!("it is the record of {}", about.id))));
                    continue;
                }
                Err(error) => {
                    problems.push((directory.about(), error));
                    continue;
                }
            };
            let places = match records::read_places(directory, &about) {
                Ok(places) => places,
                Err(error) => {
                    problems.push((directory.town(), error));
                    continue;
                }
            };
            let removed = match records::read_removed(directory) {
                Ok(removed) => removed,
                Err(error) => {
                    problems.push((directory.removed(), error));
                    continue;
                }
            };
            let now = records::read_now(directory)?;
            if let Some(session) = &about.session {
                state.threads.insert(session.clone(), about.id.clone());
                state.record_session(None, session);
            }
            // Whoever sat at the desk sat there for a turn the node that
            // stopped kept; the desk is the new node's to see afresh.
            let mut now = now;
            if let Some(now) = &mut now {
                now.desk = false;
                // Older nodes projected a retired host as a sleeping user
                // conversation. Preserve its last activity time during migration.
                if about.world_started && matches!(about.origin, Origin::User) && now.activity == Activity::Asleep {
                    now.activity = Activity::Idle;
                    records::write_now(directory, now)?;
                }
                // A departure may have been committed before the node stopped
                // halfway through putting its idle colleagues to sleep.
                if removed.is_none()
                    && now.activity == Activity::Idle
                    && let Origin::Colleague { asked_by, .. } = &about.origin
                    && state.departed.get(asked_by)?
                {
                    now.activity = Activity::Asleep;
                    records::write_now(directory, now)?;
                }
            }
            // Who is owed the turns the node stopped under: they are told how
            // those turns ended once the node is up (`unfinished`).
            let mut live = Live::new(about, places, now);
            live.removed = removed;
            state.admit(live);
            state.live.release()?;
        }
        state.live.release()?;
        Ok((
            Self {
                state: Mutex::new(state),
                changed: tokio::sync::watch::channel(0).0,
                here: Default::default(),
                speaking: Default::default(),
                paying: Default::default(),
                management: Default::default(),
            },
            problems,
        ))
    }

    /// The resident `id` lives in `town` from now on. Returns whether it
    /// moved: it may live there already.
    pub fn enter(&self, world: &WorldDir, id: &ResidentId, town: &TownName) -> io::Result<bool> {
        self.relocate(world, id, Some(town))
    }

    /// The resident `id` is in `town` from now on, or in no project. Only its
    /// `town` record changes, in one write: the move is done or not done.
    pub fn relocate(&self, world: &WorldDir, id: &ResidentId, town: Option<&TownName>) -> io::Result<bool> {
        self.active(id).map_err(io::Error::other)?;
        self.relocate_kept(world, id, town)
    }

    /// Changes only project membership, including that of an archived conversation.
    pub(crate) fn relocate_kept(&self, world: &WorldDir, id: &ResidentId, town: Option<&TownName>) -> io::Result<bool> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("no resident {id} lives here")));
        };
        let mut places = live.places.clone();
        if !places.enter(town.cloned(), Timestamp::now()) {
            return Ok(false);
        }
        let directory = world.conversation(id);
        let bytes = serde_json::to_vec_pretty(&places).map_err(io::Error::other)?;
        if let Err(failure) = fastctx_platform::atomic::publish_checked(
            &directory.town(),
            &bytes,
            fastctx_platform::Permissions::Private,
            true,
            || Ok(()),
        ) {
            if failure.published {
                live.places = places;
            }
            return Err(failure.error);
        }
        live.places = places;
        Ok(true)
    }

    /// Where residents lived before they moved.
    pub fn former(&self) -> Vec<Former> {
        let ids = self.lock().live.ids();
        ids.flat_map(|id| {
            self.lock().live.get(&id).map(|live| former(&live.about.id, &live.places)).unwrap_or_default()
        })
        .collect()
    }

    /// What the resident has been told of where it lives.
    pub fn heard(&self, id: &ResidentId) -> Option<Heard> {
        self.lock().live.get(id).and_then(|live| live.heard.clone())
    }

    pub fn set_heard(&self, id: &ResidentId, heard: Heard) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.heard = Some(heard);
        }
    }

    /// The session of `id` starts with instructions that tell it `heard` of
    /// its town: it has heard that, and is back there when it compacts.
    pub fn set_instructed(&self, id: &ResidentId, heard: Heard) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.heard = Some(heard.clone());
            live.instructed = Some(heard);
        }
    }

    /// Its host told of its context compacted, at `at`.
    pub fn compacted_at(&self, id: &ResidentId, at: Timestamp) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.compacted_at = Some(at);
            live.heard = live.instructed.clone();
        }
    }

    /// Compaction retains the opening instructions, not earlier tool-response notes.
    pub(crate) fn restore_opening_heard(&self, id: &ResidentId) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.heard = live.instructed.clone();
        }
    }

    /// When its host last told of its context compacted.
    pub fn last_compaction(&self, id: &ResidentId) -> Option<Timestamp> {
        self.lock().live.get(id).and_then(|live| live.compacted_at)
    }

    /// A tool reply was written. Concurrent replies must not move the
    /// cursor backwards or acknowledge news from a town the resident left.
    pub fn acknowledge_heard(&self, id: &ResidentId, heard: Heard) {
        if let Some(live) = self.lock().live.get_mut(id)
            && live.town() == heard.town
            && live.heard.as_ref().is_none_or(|prior| prior.town != heard.town || prior.ledger <= heard.ledger)
        {
            live.heard = Some(heard);
        }
    }

    fn lock(&self) -> StateGuard<'_> {
        StateGuard(self.state.lock().expect("never poisoned"), &self.changed)
    }

    /// Moves whenever a resident may have changed.
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Reconnects a known session or admits one the local user opened without a World identity.
    pub fn arrive(
        &self,
        world: &WorldDir,
        machine: &MachineName,
        host: Host,
        hints: Hints,
    ) -> Result<Whereabouts, Failure> {
        let mut state = self.lock();
        if let Some(resident) = hints.resident {
            return match state.live.get_mut(&resident) {
                Some(live) => {
                    if live.removed.is_some() {
                        return Err(removal::refused(&resident));
                    }
                    live.connections += 1;
                    // A thread of its own that is not the conversation's is
                    // one of its subagents, which shares its configuration.
                    let subagent = hints
                        .thread
                        .as_deref()
                        .is_some_and(|thread| live.about.session.as_deref().is_some_and(|own| own != thread));
                    Ok(Whereabouts { resident, town: live.town(), machine: None, subagent })
                }
                None => Err(Failure::new(format!(
                    "this session says it is {resident}, but {machine} has no record of that resident"
                ))),
            };
        }
        if let Some(thread) = &hints.thread
            && let Some(resident) = state.threads.get::<ResidentId>(thread)
            && let Some(live) = state.live.get_mut(&resident)
        {
            if live.removed.is_some() {
                return Err(removal::refused(&resident));
            }
            live.connections += 1;
            return Ok(Whereabouts { resident, town: live.town(), machine: None, subagent: false });
        }
        if let Some(resident) = hints.was {
            let live = state.live.get_mut(&resident).ok_or_else(|| {
                fastctx_types::authorization::Denial::new(
                    fastctx_types::authorization::DenialReason::NoSubject,
                    "the session's previous resident is not registered",
                )
            })?;
            if live.removed.is_some() {
                return Err(removal::refused(&resident));
            }
            live.connections += 1;
            return Ok(Whereabouts { resident, town: live.town(), machine: None, subagent: false });
        }
        let resident = new_id(&host.agent, &state)?;
        let about = About {
            forked_from: None,
            mcp_off: Default::default(),
            id: resident.clone(),
            host,
            machine: machine.clone(),
            born_at: Timestamp::now(),
            origin: Origin::User,
            session: hints.thread.clone(),
            world_started: false,
            native_resumed: false,
            model: None,
            acting: fastctx_types::access::Acting::Here,
            focus: Default::default(),
            home: hints.home,
            callee: None,
        };
        let places = Places::born(None, about.born_at);
        records::create(&world.conversation(&resident), &about, &places)
            .map_err(|error| Failure::new(format!("recording a new resident: {error}")))?;
        if let Some(thread) = hints.thread {
            state.record_session(None, &thread);
            state.threads.insert(thread, resident.clone());
        }
        let mut live = Live::new(about, places, None);
        live.connections = 1;
        state.admit(live);
        Ok(Whereabouts { resident, town: None, machine: None, subagent: false })
    }

    /// A connection that residents arrived on has closed. Returns those of
    /// them the user opened that are no longer here.
    pub fn left(&self, residents: &[ResidentId]) -> Vec<ResidentId> {
        let mut state = self.lock();
        let mut gone = Vec::new();
        for resident in residents {
            if let Some(live) = state.live.get_mut(resident) {
                live.connections = live.connections.saturating_sub(1);
                if !here(live) && !live.about.world_managed() {
                    gone.push(resident.clone());
                }
            }
        }
        gone
    }

    /// Records a resident the World started, with what it is doing.
    pub fn create(&self, world: &WorldDir, about: About, town: Option<TownName>, now: Now) -> io::Result<()> {
        let directory = world.conversation(&about.id);
        let places = Places::born(town, about.born_at);
        records::create(&directory, &about, &places)?;
        // Files attached before it was opened are its own now.
        match std::fs::remove_file(directory.draft()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let recorded = records::write_now(&directory, &now);
        let mut state = self.lock();
        if let Some(session) = &about.session {
            state.threads.insert(session.clone(), about.id.clone());
            state.record_session(None, session);
        }
        // The durable identity already exists even if its initial activity
        // could not be written. Retain it so a retry cannot create a second one.
        state.admit(Live::new(about, places, recorded.as_ref().ok().map(|()| now)));
        recorded
    }

    /// The resident whose Claude session has the inbox address `inbox`.
    pub fn resident_of_inbox(&self, inbox: &str) -> Option<ResidentId> {
        self.lock().inboxes.get(&inbox)
    }

    /// The mod's inbox address identifies commands from the host's native
    /// shell. MCP calls never use it as identity evidence.
    pub fn set_inbox(&self, id: &ResidentId, inbox: String) {
        let mut state = self.lock();
        let prior = state.live.get(id).and_then(|live| live.inbox.clone());
        if let Some(prior) = prior
            && state.inboxes.get::<ResidentId>(&prior).as_ref() == Some(id)
        {
            state.inboxes.remove(&prior);
        }
        state.inboxes.insert(&inbox, id);
        if let Some(live) = state.live.get_mut(id) {
            live.inbox = Some(inbox);
        }
    }

    pub(crate) fn host_present(&self, id: &ResidentId) -> bool {
        self.lock().live.get(id).is_some_and(|live| live.host_present)
    }

    /// Updates host presence and says whether a user-opened resident left.
    pub(crate) fn set_host_present(&self, id: &ResidentId, present: bool) -> bool {
        if let Some(live) = self.lock().live.get_mut(id) {
            let was_here = here(live);
            live.host_present = present;
            return was_here && !here(live) && !live.about.world_managed();
        }
        false
    }

    /// The resident's session is `session` on its host: its `about` says so
    /// from now on, with the model and version the host gave.
    pub fn set_session(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        session: &str,
        model: Option<String>,
        version: Option<String>,
    ) -> io::Result<()> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Ok(());
        };
        if live.removed.is_some() {
            return Ok(());
        }
        let mut about = live.about.clone();
        let prior_session = about.session.clone();
        about.session = Some(session.to_string());
        if model.is_some() {
            about.model = model;
        }
        if let Some(version) = version {
            about.host.version = version;
        }
        if about == live.about {
            return Ok(());
        }
        records::write_about(&world.conversation(id), &about)?;
        live.about = about;
        state.record_session(prior_session.as_deref(), session);
        state.threads.insert(session.to_string(), id.clone());
        Ok(())
    }

    /// Persists the restoration contract before the first native resumption.
    pub(crate) fn set_native_resumed(&self, world: &WorldDir, id: &ResidentId) -> io::Result<()> {
        let mut state=self.lock();
        let live=state.live.get_mut(id).ok_or_else(||io::Error::other("unknown conversation"))?;
        if live.about.native_resumed {return Ok(());}
        let mut about=live.about.clone();
        about.native_resumed=true;
        records::write_about(&world.conversation(id),&about)?;
        live.about=about;
        Ok(())
    }

    /// The machines and folders the user points the resident `id` to from
    /// now on; whether they changed.
    pub fn set_focus(&self, world: &WorldDir, id: &ResidentId, focus: Focus) -> io::Result<bool> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Ok(false);
        };
        if live.removed.is_some() || live.about.focus == focus {
            return Ok(false);
        }
        let mut about = live.about.clone();
        about.focus = focus;
        records::write_about(&world.conversation(id), &about)?;
        live.about = about;
        Ok(true)
    }

    pub fn set_mcp_off(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        off: std::collections::BTreeSet<String>,
    ) -> io::Result<()> {
        let mut state = self.lock();
        let live = state.live.get_mut(id).ok_or_else(|| io::Error::other("unknown conversation"))?;
        let mut about = live.about.clone();
        about.mcp_off = off;
        records::write_about(&world.conversation(id), &about)?;
        live.about = about;
        Ok(())
    }

    /// A new id for a resident of `agent`, unused on this machine.
    pub fn new_id(&self, agent: &AgentName) -> Result<ResidentId, Failure> {
        new_id(agent, &self.lock())
    }

    /// Whether the resident `id` lives on this machine, removed or not.
    pub fn knows(&self, id: &ResidentId) -> bool {
        self.lock().live.contains_key(id)
    }

    /// The town of the resident `id` living on this machine; `None` when no
    /// such resident lives here.
    pub fn town_of(&self, id: &ResidentId) -> Option<Option<TownName>> {
        self.lock().live.get(id).map(Live::town)
    }

    pub fn about(&self, id: &ResidentId) -> Option<(About, Option<TownName>)> {
        self.lock().live.get(id).map(|live| (self.placed(live.about.clone()), live.town()))
    }

    /// This machine is called `name` now (`world-network.md`, 机器的名字).
    pub fn set_here(&self, name: fastctx_types::MachineName) {
        *self.here.write().expect("never poisoned") = Some(name);
        self.changed.send_modify(|changes| *changes = changes.wrapping_add(1));
    }

    /// `about` of a resident living here, on this machine as it is called
    /// now.
    fn placed(&self, mut about: About) -> About {
        if let Some(here) = &*self.here.read().expect("never poisoned") {
            about.machine = here.clone();
        }
        about
    }

    pub fn now(&self, id: &ResidentId) -> Option<Now> {
        self.lock().live.get(id).and_then(|live| live.now.clone())
    }

    pub fn resident_of_thread(&self, thread: &str) -> Option<ResidentId> {
        self.lock().threads.get(&thread)
    }

    pub fn records_for_session(&self, session: &str) -> u64 {
        self.lock().current_sessions.get(session).unwrap_or(0)
    }

    pub fn speaking(
        &self,
        id: &ResidentId,
    ) -> Option<crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<()>>> {
        self.lock().live.contains_key(id).then(|| self.speaking.get_or_insert_with(id, || crate::ranked::speaking(id)))
    }

    pub fn paying(
        &self,
        id: &ResidentId,
    ) -> Option<crate::leases::Lease<ResidentId, fastctx_platform::doing::Mutex<()>>> {
        self.lock().live.contains_key(id).then(|| self.paying.get_or_insert_with(id, || crate::ranked::paying(id)))
    }

    /// Every resident on record.
    pub fn everyone(&self) -> impl Iterator<Item = OnRecord> + '_ {
        let ids = self.lock().live.ids();
        ids.filter_map(|id| self.on_record(&id))
    }

    /// One resident on record, read alone rather than found among everyone.
    pub fn on_record(&self, id: &ResidentId) -> Option<OnRecord> {
        let mut state = self.lock();
        state.live.get(id).map(|live| OnRecord {
            id: id.clone(),
            removed: live.removed,
            town: live.town(),
            here: here(live),
            about: self.placed(live.about.clone()),
            now: live.now.clone(),
        })
    }

    /// Codex residents the World holds that were working when the node
    /// stopped, with the turn they ran (empty when none had started): their
    /// app-server stopped with the node. A Claude session runs on under
    /// Claude Code's own supervisor, and its turn ends as it ends.
    pub fn unfinished(&self) -> Vec<(ResidentId, String)> {
        let ids = self.lock().live.ids();
        ids.flat_map(|id| {
            let mut state = self.lock();
            let Some(live) = state.live.get(&id).filter(|live| live.removed.is_none()) else { return Vec::new() };
            let mut turns: BTreeSet<String> = live.owed.keys().cloned().collect();
            if let Some(now) = &live.now
                && now.activity == Activity::Busy
                && let Some(turn) = &now.turn
            {
                turns.insert(turn.clone());
            }
            turns.into_iter().map(|turn| (live.about.id.clone(), turn)).collect::<Vec<_>>()
        })
        .collect()
    }

    /// Durably records a remote resident leaving or returning. Idle
    /// colleagues sleep before a departure can be acknowledged. `letter`
    /// names its hub and stable delivery id; an ask has no letter.
    pub fn set_departed(
        &self,
        world: &WorldDir,
        asker: &Address,
        departed: bool,
        letter: Option<(&str, &crate::letters::Identity)>,
    ) -> io::Result<()> {
        let mut state = self.lock();
        if state.departed.set(asker, departed, letter)? {
            state.sleep_colleagues(world, asker)?;
        }
        Ok(())
    }

    /// Puts only idle colleagues to sleep, under the same lock that starts
    /// and ends their turns.
    pub fn sleep_colleagues(&self, world: &WorldDir, asker: &Address) -> io::Result<()> {
        self.lock().sleep_colleagues(world, asker)
    }

    /// Changes a resident's `now` and writes it through. One with no `now`
    /// yet starts idle.
    pub fn change_now(&self, world: &WorldDir, id: &ResidentId, change: impl FnOnce(&mut Now)) -> io::Result<()> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Ok(());
        };
        if live.removed.is_some() {
            return Ok(());
        }
        let mut now = live.now.clone().unwrap_or_else(|| idle(Timestamp::now()));
        change(&mut now);
        records::write_now(&world.conversation(id), &now)?;
        live.now = Some(now);
        Ok(())
    }

    /// The session runs `turn`: what was told about an earlier one is not
    /// told about it.
    pub fn turn_started(&self, world: &WorldDir, id: &ResidentId, turn: &str, at: Timestamp) -> io::Result<()> {
        self.with_live(world, id, |_live, now| {
            enter(now, turn);
            now.since = at;
            now.last_compacted = false;
        })
    }

    /// Words of `readers` went into `turn`: each is owed its end.
    pub fn owe(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        turn: &str,
        readers: &[Address],
        active: bool,
    ) -> io::Result<()> {
        self.with_proof(world, id, |live, now| {
            let already = live.owed.contains_key(turn);
            live.owed.entry(turn.to_string()).or_default().extend(readers.iter().cloned());
            if !already && active {
                enter(now, turn);
            }
            now.owed = owed(live);
        })
    }

    /// Who is owed `turn`'s end, as it stands.
    pub fn readers(&self, id: &ResidentId, turn: &str) -> Vec<Address> {
        self.lock()
            .live
            .get(id)
            .and_then(|live| live.owed.get(turn))
            .map(|readers| readers.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Who the resident is working for: those owed the turn it runs, or,
    /// between turns, those its latest turn anyone was owed was for.
    /// Colleagues incur debt only from explicit messages, never inherited background work.
    pub fn working_for(&self, id: &ResidentId) -> Vec<Address> {
        let mut state = self.lock();
        let Some(live) = state.live.get(id) else { return Vec::new() };
        let Some(now) = &live.now else { return Vec::new() };
        if matches!(live.about.origin, Origin::Colleague { .. }) {
            return Vec::new();
        }
        if now.activity == Activity::Busy
            && let Some(turn) = &now.turn
        {
            return live.owed.get(turn).map(|readers| readers.iter().cloned().collect()).unwrap_or_default();
        }
        now.answering.clone()
    }

    /// Who was owed `turn`'s end, taken now that it ended; the resident is
    /// idle, or asleep when whoever it works for is gone, unless words
    /// meanwhile started another turn. `trouble` says the turn did not end
    /// as it should.
    pub fn take_owed(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        turn: &str,
        here_machine: &MachineName,
        trouble: Option<Trouble>,
        heard_user: bool,
    ) -> io::Result<Vec<Address>> {
        let mut state = self.lock();
        let Some(origin) = state.live.get(id).map(|live| live.about.origin.clone()) else {
            return Ok(Vec::new());
        };
        // Presence and the turn's final activity are decided in one
        // transaction with departures and returns.
        let requester_here = match origin {
            Origin::Colleague { asked_by, .. } if asked_by.machine == *here_machine => {
                state.live.get(&asked_by.resident).is_some_and(here)
            }
            Origin::Colleague { asked_by, .. } => !state.departed.get(&asked_by)?,
            Origin::User => true,
        };
        let mut readers = Vec::new();
        state.change_live(
            world,
            id,
            |live, now| {
                let current = now.turn.as_deref() == Some(turn)
                    || (now.turn.is_none() && now.activity == Activity::Busy && turn.is_empty());
                readers = live.owed.remove(turn).unwrap_or_default().into_iter().collect();
                // Words may meanwhile have started the next turn, whose state
                // stands.
                if current {
                    now.last_turn = Some(turn.to_string());
                    now.last_heard_user = Some(heard_user);
                    now.turn = None;
                    now.waiting_for_user = false;
                    now.waiting_since = None;
                    now.desk = false;
                    now.desk_since = None;
                    now.activity = if requester_here { Activity::Idle } else { Activity::Asleep };
                    now.since = Timestamp::now();
                }
                if !readers.is_empty() {
                    now.answering = readers.clone();
                }
                now.owed = owed(live);
                if current {
                    now.trouble = trouble;
                }
            },
            true,
        )?;
        Ok(readers)
    }

    fn with_live(&self, world: &WorldDir, id: &ResidentId, change: impl FnOnce(&mut Live, &mut Now)) -> io::Result<()> {
        self.change_live(world, id, change, false)
    }

    fn with_proof(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        change: impl FnOnce(&mut Live, &mut Now),
    ) -> io::Result<()> {
        self.change_live(world, id, change, true)
    }

    fn change_live(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        change: impl FnOnce(&mut Live, &mut Now),
        proven: bool,
    ) -> io::Result<()> {
        self.lock().change_live(world, id, change, proven)
    }
}

impl State {
    fn sleep_colleagues(&mut self, world: &WorldDir, asker: &Address) -> io::Result<()> {
        let colleagues = self.colleagues.get(asker).cloned().unwrap_or_default();
        let mut failures = Vec::new();
        for colleague in colleagues {
            let idle = self.live.get(&colleague).is_some_and(|live| {
                live.removed.is_none() && live.now.as_ref().is_some_and(|now| now.activity == Activity::Idle)
            });
            if idle
                && let Err(error) = self.change_live(
                    world,
                    &colleague,
                    |_, now| {
                        now.activity = Activity::Asleep;
                    },
                    false,
                )
            {
                failures.push(format!("recording that {colleague} sleeps: {error}"));
            }
        }
        if failures.is_empty() { Ok(()) } else { Err(io::Error::other(failures.join("; "))) }
    }

    fn change_live(
        &mut self,
        world: &WorldDir,
        id: &ResidentId,
        change: impl FnOnce(&mut Live, &mut Now),
        proven: bool,
    ) -> io::Result<()> {
        let Some(live) = self.live.get_mut(id) else {
            return Ok(());
        };
        if live.removed.is_some() {
            return Ok(());
        }
        let mut now = live.now.clone().unwrap_or_else(|| idle(Timestamp::now()));
        let previous_owed = live.owed.clone();
        change(live, &mut now);
        if let Err(error) = records::write_now(&world.conversation(id), &now) {
            // A receipt or terminal record is authoritative even while its
            // replaceable now projection cannot be written.
            if proven {
                live.now = Some(now);
            } else {
                live.owed = previous_owed;
            }
            return Err(error);
        }
        live.now = Some(now);
        Ok(())
    }
}

fn here(live: &Live) -> bool {
    if live.removed.is_some() {
        return false;
    }
    if live.about.world_managed() {
        (live.about.native_resumed && live.connections > 0) || live.now.as_ref().is_none_or(|now| now.activity != Activity::Asleep)
    } else {
        live.host_present || live.connections > 0
    }
}

fn enter(now: &mut Now, turn: &str) {
    if now.turn.as_deref() != Some(turn) || now.activity != Activity::Busy {
        now.since = Timestamp::now();
        now.waiting_for_user = false;
        now.waiting_since = None;
    }
    now.turn = Some(turn.to_string());
    now.activity = Activity::Busy;
}

/// Who is owed each running turn, as `now` keeps it.
fn owed(live: &Live) -> BTreeMap<String, Vec<Address>> {
    live.owed
        .iter()
        .filter(|(_, readers)| !readers.is_empty())
        .map(|(turn, readers)| (turn.clone(), readers.iter().cloned().collect()))
        .collect()
}

pub fn idle(since: Timestamp) -> Now {
    Now {
        activity: Activity::Idle,
        since,
        turn: None,
        last_turn: None,
        owed: BTreeMap::new(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        last_compacted: false,
        last_heard_user: None,
        asked: None,
        told: None,
    }
}

/// A fresh id for a resident of `agent`: its name and four hex digits,
/// unused on this machine, and by no conversation deleted from it lately.
fn new_id(agent: &AgentName, state: &State) -> Result<ResidentId, Failure> {
    loop {
        let mut bytes = [0u8; 2];
        getrandom::fill(&mut bytes).map_err(|error| Failure::new(format!("choosing a resident id: {error}")))?;
        let id = ResidentId::try_from(format!("{agent}-{:02x}{:02x}", bytes[0], bytes[1]))
            .expect("an agent name and four hex digits");
        if !state.live.contains_key(&id) && state.tombstones.of(&id).is_none() {
            return Ok(id);
        }
    }
}

/// Each time the resident left a project, as `fastctx w` lists it there.
fn former(id: &ResidentId, places: &Places) -> Vec<Former> {
    places
        .departures()
        .into_iter()
        .map(|(town, moved)| Former { id: id.clone(), town, to: moved.to, at: moved.at })
        .collect()
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;

    #[test]
    fn departed_residents_keep_identity_and_history_without_runtime_entries() {
        let root = std::env::temp_dir().join(format!("resident-lifetimes-{}", std::process::id()));
        let world = WorldDir::under(&root);
        let (residents, problems) = Residents::load(&world).unwrap();
        assert!(problems.is_empty());
        let machine = "private-node".parse().unwrap();
        let host = Host { agent: "codex".parse().unwrap(), version: "fixture".into(), client: "fixture".into() };
        let hints = |index| Hints {
            host_process: None,
            resident: None,
            thread: Some(format!("thread-{index}")),
            was: None,
            tool_use_id: None,
            home: None,
        };
        for index in 0..512 {
            let arrived = residents.arrive(&world, &machine, host.clone(), hints(index)).unwrap();
            let id = &arrived.resident;
            residents
                .set_heard(id, Heard { scratch: None, context: None, user_prompt: None, town: None, ledger: index });
            residents.set_inbox(id, format!("old-{index}"));
            residents.set_inbox(id, format!("new-{index}"));
            assert!(residents.resident_of_inbox(&format!("old-{index}")).is_none());
            residents.left(std::slice::from_ref(id));
            assert_eq!(residents.state.lock().unwrap().live.cached(), 0);
            let held = residents.speaking(id).unwrap();
            let again = residents.speaking(id).unwrap();
            assert!(std::ptr::eq(&*held, &*again));
            assert_eq!(residents.arrive(&world, &machine, host.clone(), hints(index)).unwrap().resident, *id);
            assert_eq!(residents.heard(id).unwrap().ledger, index);
            residents.left(std::slice::from_ref(id));
        }
        assert_eq!(residents.everyone().count(), 512);
        assert_eq!(residents.state.lock().unwrap().live.cached(), 0);
        let removed = residents.resident_of_thread("thread-0").unwrap();
        residents.remove(&world, &removed).unwrap();
        assert!(residents.arrive(&world, &machine, host.clone(), hints(0)).is_err());
        drop(residents);
        let (restored, problems) = Residents::load(&world).unwrap();
        assert!(problems.is_empty());
        assert_eq!(restored.everyone().count(), 512);
        assert!(restored.arrive(&world, &machine, host, hints(0)).is_err());
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }
}
