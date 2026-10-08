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
//! A resident has a directory in each town it lived in; all but the one it
//! lives in now are marked as left (`records::move_to`).

mod records;
mod removal;
pub use removal::removal;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{About, Activity, Hints, Host, Moved, Now, Origin, Trouble, Whereabouts};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

pub use records::{append_message, last_action, read_about, read_now, record_action, record_seen};

pub struct Residents {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    live: HashMap<ResidentId, Live>,
    /// Codex thread id to the resident it is.
    threads: HashMap<String, ResidentId>,
    /// Residents of other machines that asked for colleagues here and have
    /// since left, as their machines wrote. They come back by asking or
    /// speaking again.
    departed: HashSet<Address>,
    /// Where residents lived before they moved.
    former: Vec<Former>,
}

/// The directories a resident has, one in each town it lived in, and for
/// each where it moved from there.
type Homes = Vec<(Option<TownName>, Option<Moved>)>;

/// A town a resident lived in and moved away from.
#[derive(Clone)]
pub struct Former {
    pub id: ResidentId,
    pub town: Option<TownName>,
    pub to: Option<TownName>,
    pub at: Timestamp,
}

/// What a resident has been told of the town it lives in: the town, and
/// how far into its ledger.
#[derive(Clone, PartialEq, Eq)]
pub struct Heard {
    pub town: Option<TownName>,
    pub ledger: u64,
}

struct Live {
    removed: Option<Timestamp>,
    management: Arc<tokio::sync::RwLock<()>>,
    about: About,
    town: Option<TownName>,
    /// A Claude session's inbox address, for commands from its native shell.
    inbox: Option<String>,
    /// Open connections the resident arrived on.
    connections: usize,
    /// Presence reported by a Claude mod, independent of its MCP connections.
    claude_here: bool,
    now: Option<Now>,
    /// For each turn running, whose words went into it.
    owed: HashMap<String, BTreeSet<Address>>,
    /// Held while words go into its session until the turn they joined is
    /// known and owed, and while a turn's end is taken: so a turn's end goes
    /// to exactly those whose words went into it.
    speaking: Arc<tokio::sync::Mutex<()>>,
    paying: Arc<tokio::sync::Mutex<()>>,
    /// `None` until it is told who and where it is.
    heard: Option<Heard>,
}

impl Live {
    fn new(about: About, town: Option<TownName>, now: Option<Now>) -> Self {
        let owed = now
            .iter()
            .flat_map(|now| now.owed.iter())
            .map(|(turn, readers)| (turn.clone(), readers.iter().cloned().collect()))
            .collect();
        Self {
            removed: None,
            management: Arc::new(tokio::sync::RwLock::new(())),
            about,
            town,
            inbox: None,
            connections: 0,
            claude_here: false,
            now,
            owed,
            speaking: Arc::new(tokio::sync::Mutex::new(())),
            paying: Arc::new(tokio::sync::Mutex::new(())),
            heard: None,
        }
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
    /// Reads every resident this machine has a record of. A record that
    /// cannot be read is reported and left as it is.
    pub fn load(world: &WorldDir) -> io::Result<(Self, Vec<(PathBuf, io::Error)>)> {
        removal::recover(world)?;
        let mut state = State::default();
        let mut problems = Vec::new();
        // Each resident's directories, one in each town it lived in.
        let mut homes: BTreeMap<ResidentId, Homes> = BTreeMap::new();
        for parent in world.resident_parents()? {
            let town = town_of(world, &parent);
            let entries = match std::fs::read_dir(&parent) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let path = entry?.path();
                // A visitor's directory holds what a resident of another
                // machine did here; it has no `about`.
                let Some(id) = path.file_name().and_then(|name| ResidentId::try_from(name.to_str()?).ok()) else {
                    continue;
                };
                let directory = world.resident(town.as_ref(), &id);
                if !directory.about().exists() {
                    continue;
                }
                match records::read_moved(&directory) {
                    Ok(moved) => homes.entry(id).or_default().push((town.clone(), moved)),
                    Err(error) => problems.push((directory.moved(), error)),
                }
            }
        }
        for (id, homes) in homes {
            let town = match live_town(world, &id, &homes) {
                Ok(town) => town,
                Err(error) => {
                    problems.push((world.resident(None, &id).path().to_path_buf(), error));
                    continue;
                }
            };
            for (left, moved) in &homes {
                if *left != town && removal(world, &id)?.is_none() {
                    records::carry_obligations(world, &id, left.as_ref(), town.as_ref())?;
                }
                if let Some(moved) = moved
                    && *left != town
                {
                    state.former.push(Former {
                        id: id.clone(),
                        town: left.clone(),
                        to: moved.to.clone(),
                        at: moved.at,
                    });
                }
            }
            let directory = world.resident(town.as_ref(), &id);
            let about = match records::read_about(&directory) {
                Ok(about) => about,
                Err(error) => {
                    problems.push((directory.about(), error));
                    continue;
                }
            };
            let now = records::read_now(&directory)?;
            if let Some(session) = &about.session {
                state.threads.insert(session.clone(), about.id.clone());
            }
            // Whoever sat at the desk sat there for a turn the node that
            // stopped kept; the desk is the new node's to see afresh.
            let now = now.map(|mut now| {
                now.desk = false;
                now
            });
            // Who is owed the turns the node stopped under: they are told how
            // those turns ended once the node is up (`unfinished`).
            let mut live = Live::new(about, town.clone(), now);
            live.removed = removal(world, &id)?;
            state.live.insert(live.about.id.clone(), live);
        }
        Ok((Self { state: Mutex::new(state) }, problems))
    }

    /// The resident `id` lives in `town` from now on. Returns whether it
    /// moved: it may live there already.
    pub fn enter(&self, world: &WorldDir, id: &ResidentId, town: &TownName) -> io::Result<bool> {
        self.relocate(world, id, Some(town))
    }

    pub fn relocate(&self, world: &WorldDir, id: &ResidentId, town: Option<&TownName>) -> io::Result<bool> {
        self.active(id).map_err(io::Error::other)?;
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("no resident {id} lives here")));
        };
        if live.town.as_ref() == town {
            return Ok(false);
        }
        let from = live.town.clone();
        records::move_to(world, id, from.as_ref(), town, &live.about, live.now.as_ref())?;
        live.town = town.cloned();
        state.former.retain(|former| !(former.id == *id && former.town.as_ref() == town));
        state.former.push(Former { id: id.clone(), town: from, to: town.cloned(), at: Timestamp::now() });
        Ok(true)
    }

    /// Where residents lived before they moved.
    pub fn former(&self) -> Vec<Former> {
        self.lock().former.clone()
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

    /// A tool reply was written. Concurrent replies must not move the
    /// cursor backwards or acknowledge news from a town the resident left.
    pub fn acknowledge_heard(&self, id: &ResidentId, heard: Heard) {
        if let Some(live) = self.lock().live.get_mut(id)
            && live.town == heard.town
            && live.heard.as_ref().is_none_or(|prior| prior.town != heard.town || prior.ledger <= heard.ledger)
        {
            live.heard = Some(heard);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("never poisoned")
    }

    /// The resident a session is, made now if it is new.
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
                    Ok(Whereabouts { resident, town: live.town.clone() })
                }
                None => Err(Failure::new(format!(
                    "this session says it is {resident}, but {machine} has no record of that resident"
                ))),
            };
        }
        if let Some(thread) = &hints.thread
            && let Some(resident) = state.threads.get(thread).cloned()
            && let Some(live) = state.live.get_mut(&resident)
        {
            if live.removed.is_some() {
                return Err(removal::refused(&resident));
            }
            live.connections += 1;
            return Ok(Whereabouts { resident, town: live.town.clone() });
        }
        if let Some(resident) = hints.was
            && let Some(live) = state.live.get_mut(&resident)
        {
            if live.removed.is_some() {
                return Err(removal::refused(&resident));
            }
            live.connections += 1;
            return Ok(Whereabouts { resident, town: live.town.clone() });
        }
        let resident = new_id(&host.agent, &state)?;
        let about = About {
            id: resident.clone(),
            host,
            machine: machine.clone(),
            born_at: Timestamp::now(),
            origin: Origin::User,
            session: hints.thread.clone(),
            world_started: false,
            model: None,
        };
        records::create(&world.resident(None, &resident), &about)
            .map_err(|error| Failure::new(format!("recording a new resident: {error}")))?;
        if let Some(thread) = hints.thread {
            state.threads.insert(thread, resident.clone());
        }
        let mut live = Live::new(about, None, None);
        live.connections = 1;
        state.live.insert(resident.clone(), live);
        Ok(Whereabouts { resident, town: None })
    }

    /// A connection that residents arrived on has closed. Returns those of
    /// them the user opened that are no longer here.
    pub fn left(&self, residents: &[ResidentId]) -> Vec<ResidentId> {
        let mut state = self.lock();
        let mut gone = Vec::new();
        for resident in residents {
            if let Some(live) = state.live.get_mut(resident) {
                live.connections = live.connections.saturating_sub(1);
                if !here(live) && !live.about.world_started {
                    gone.push(resident.clone());
                }
            }
        }
        gone
    }

    /// Records a resident the World started, with what it is doing.
    pub fn create(&self, world: &WorldDir, about: About, town: Option<TownName>, now: Now) -> io::Result<()> {
        let directory = world.resident(town.as_ref(), &about.id);
        records::create(&directory, &about)?;
        let recorded = records::write_now(&directory, &now);
        let mut state = self.lock();
        if let Some(session) = &about.session {
            state.threads.insert(session.clone(), about.id.clone());
        }
        // The durable identity already exists even if its initial activity
        // could not be written. Retain it so a retry cannot create a second one.
        state.live.insert(about.id.clone(), Live::new(about, town, recorded.as_ref().ok().map(|()| now)));
        recorded
    }

    /// The resident whose Claude session has the inbox address `inbox`.
    pub fn resident_of_inbox(&self, inbox: &str) -> Option<ResidentId> {
        self.lock().live.values().find(|live| live.inbox.as_deref() == Some(inbox)).map(|live| live.about.id.clone())
    }

    /// The mod's inbox address identifies commands from the host's native
    /// shell. MCP calls never use it as identity evidence.
    pub fn set_inbox(&self, id: &ResidentId, inbox: String) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.inbox = Some(inbox);
        }
    }

    /// Updates mod presence and says whether a user-opened resident left.
    pub fn set_claude_here(&self, id: &ResidentId, present: bool) -> bool {
        if let Some(live) = self.lock().live.get_mut(id) {
            let was_here = here(live);
            live.claude_here = present;
            return was_here && !here(live) && !live.about.world_started;
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
        records::write_about(&world.resident(live.town.as_ref(), id), &about)?;
        live.about = about;
        state.threads.insert(session.to_string(), id.clone());
        Ok(())
    }

    /// A new id for a resident of `agent`, unused on this machine.
    pub fn new_id(&self, agent: &AgentName) -> Result<ResidentId, Failure> {
        new_id(agent, &self.lock())
    }

    /// The town of the resident `id` living on this machine; `None` when no
    /// such resident lives here.
    pub fn town_of(&self, id: &ResidentId) -> Option<Option<TownName>> {
        self.lock().live.get(id).map(|live| live.town.clone())
    }

    pub fn about(&self, id: &ResidentId) -> Option<(About, Option<TownName>)> {
        self.lock().live.get(id).map(|live| (live.about.clone(), live.town.clone()))
    }

    pub fn now(&self, id: &ResidentId) -> Option<Now> {
        self.lock().live.get(id).and_then(|live| live.now.clone())
    }

    pub fn resident_of_thread(&self, thread: &str) -> Option<ResidentId> {
        self.lock().threads.get(thread).cloned()
    }

    pub fn speaking(&self, id: &ResidentId) -> Option<Arc<tokio::sync::Mutex<()>>> {
        self.lock().live.get(id).map(|live| live.speaking.clone())
    }

    pub fn paying(&self, id: &ResidentId) -> Option<Arc<tokio::sync::Mutex<()>>> {
        self.lock().live.get(id).map(|live| live.paying.clone())
    }

    /// Every resident on record.
    pub fn everyone(&self) -> Vec<OnRecord> {
        let state = self.lock();
        state
            .live
            .iter()
            .map(|(id, live)| OnRecord {
                id: id.clone(),
                removed: live.removed,
                town: live.town.clone(),
                here: here(live),
                about: live.about.clone(),
                now: live.now.clone(),
            })
            .collect()
    }

    /// Codex residents the World holds that were working when the node
    /// stopped, with the turn they ran (empty when none had started): their
    /// app-server stopped with the node. A Claude session runs on under
    /// Claude Code's own supervisor, and its turn ends as it ends.
    pub fn unfinished(&self) -> Vec<(ResidentId, String)> {
        self.lock()
            .live
            .values()
            .filter(|live| live.removed.is_none())
            .flat_map(|live| {
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

    /// Every resident the World started whose words come from `asker`.
    pub fn colleagues_of(&self, asker: &Address) -> Vec<ResidentId> {
        self.lock()
            .live
            .values()
            .filter(|live| matches!(&live.about.origin, Origin::Colleague { asked_by, .. } if asked_by == asker))
            .map(|live| live.about.id.clone())
            .collect()
    }

    /// Whether the resident a colleague works for is here: one of this
    /// machine that has not left, or one of another machine whose machine
    /// has not written that it left.
    pub fn requester_here(&self, asker: &Address, here_machine: &MachineName) -> bool {
        let state = self.lock();
        if asker.machine == *here_machine {
            state.live.get(&asker.resident).is_some_and(here)
        } else {
            !state.departed.contains(asker)
        }
    }

    /// A resident of another machine left (`departed`) or came back.
    pub fn set_departed(&self, asker: Address, departed: bool) {
        let mut state = self.lock();
        if departed {
            state.departed.insert(asker);
        } else {
            state.departed.remove(&asker);
        }
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
        records::write_now(&world.resident(live.town.as_ref(), id), &now)?;
        live.now = Some(now);
        Ok(())
    }

    /// The session runs `turn`: what was told about an earlier one is not
    /// told about it.
    pub fn turn_started(&self, world: &WorldDir, id: &ResidentId, turn: &str, at: Timestamp) -> io::Result<()> {
        self.with_live(world, id, |_live, now| {
            enter(now, turn);
            now.since = at;
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
    pub fn working_for(&self, id: &ResidentId) -> Vec<Address> {
        let state = self.lock();
        let Some(live) = state.live.get(id) else { return Vec::new() };
        let Some(now) = &live.now else { return Vec::new() };
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
        requester_here: bool,
        trouble: Option<Trouble>,
    ) -> io::Result<Vec<Address>> {
        let mut readers = Vec::new();
        self.with_proof(world, id, |live, now| {
            let current = now.turn.as_deref() == Some(turn)
                || (now.turn.is_none() && now.activity == Activity::Busy && turn.is_empty());
            readers = live.owed.remove(turn).unwrap_or_default().into_iter().collect();
            // Words may meanwhile have started the next turn, whose state
            // stands.
            if current {
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
        })?;
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
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Ok(());
        };
        if live.removed.is_some() {
            return Ok(());
        }
        let mut now = live.now.clone().unwrap_or_else(|| idle(Timestamp::now()));
        let previous_owed = live.owed.clone();
        change(live, &mut now);
        if let Err(error) = records::write_now(&world.resident(live.town.as_ref(), id), &now) {
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
    if live.about.world_started {
        live.now.as_ref().is_none_or(|now| now.activity != Activity::Asleep)
    } else {
        live.claude_here || live.connections > 0
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
        owed: BTreeMap::new(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
    }
}

/// A fresh id for a resident of `agent`: its name and four hex digits,
/// unused on this machine.
fn new_id(agent: &AgentName, state: &State) -> Result<ResidentId, Failure> {
    loop {
        let mut bytes = [0u8; 2];
        getrandom::fill(&mut bytes).map_err(|error| Failure::new(format!("choosing a resident id: {error}")))?;
        let id = ResidentId::try_from(format!("{agent}-{:02x}{:02x}", bytes[0], bytes[1]))
            .expect("an agent name and four hex digits");
        if !state.live.contains_key(&id) {
            return Ok(id);
        }
    }
}

/// The town a resident lives in: the one of its directories not marked as
/// left. A move cut short after it marked the old directory left none such;
/// it is finished now, to the town of the latest move.
fn live_town(world: &WorldDir, id: &ResidentId, homes: &Homes) -> io::Result<Option<TownName>> {
    let unmarked: Vec<&Option<TownName>> =
        homes.iter().filter(|(_, moved)| moved.is_none()).map(|(town, _)| town).collect();
    match unmarked.as_slice() {
        [town] => Ok((*town).clone()),
        [] => {
            let (from, moved) = homes
                .iter()
                .filter_map(|(town, moved)| Some((town, moved.as_ref()?)))
                .max_by_key(|(_, moved)| moved.at)
                .ok_or_else(|| io::Error::other(format!("{id} has no directory")))?;
            let about = records::read_about(&world.resident(from.as_ref(), id))?;
            records::move_to(world, id, from.as_ref(), moved.to.as_ref(), &about, None)?;
            Ok(moved.to.clone())
        }
        several => Err(io::Error::other(format!(
            "{id} has a home in more than one town ({}); each but one should say where it moved",
            several
                .iter()
                .map(|town| town.as_ref().map_or("outside", |town| town.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The town a directory of residents belongs to, from where it lies.
fn town_of(world: &WorldDir, parent: &std::path::Path) -> Option<TownName> {
    let towns = world.path().join("towns");
    let town = parent.parent()?.strip_prefix(&towns).ok()?;
    TownName::try_from(town.to_str()?).ok()
}
