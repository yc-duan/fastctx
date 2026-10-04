//! Who lives on this machine: finding a session's resident or making one,
//! knowing who is here, and, for a session whose host tells the World when
//! it works, what it is doing and who is owed the end of its turn.
//!
//! A resident the World started is named in its environment. A Codex
//! session is recognised by its thread, which stays the same across the
//! several MCP connections Codex may open for it. Any other session is a
//! resident of its own from its first call. A session the user opened is
//! here while a connection it arrived on stays open; one the World started
//! is here unless it is asleep.
//!
//! What a session owes is kept in memory and written through to its `now`
//! on every change, so a node that restarts knows whom to tell.

mod records;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{About, Activity, Hints, Host, Now, Origin, Trouble, Whereabouts};
use fastctx_types::session::HealthKind;
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

pub use records::{append_message, last_action, record_action, record_seen};

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
}

struct Live {
    about: About,
    town: Option<TownName>,
    /// A Claude session's inbox address, from its MCP connection.
    inbox: Option<String>,
    /// Open connections the resident arrived on.
    connections: usize,
    now: Option<Now>,
    /// For each turn running or about to, whose words went into it.
    owed: HashMap<String, BTreeSet<Address>>,
    /// The urgent kinds of health already told to those owed the running
    /// turn's end: each is told once a turn.
    told: BTreeSet<HealthKind>,
    /// Held while words go into its session until the turn they joined is
    /// known and owed, and while a turn's end is taken: so a turn's end goes
    /// to exactly those whose words went into it.
    speaking: Arc<tokio::sync::Mutex<()>>,
}

impl Live {
    fn new(about: About, town: Option<TownName>, now: Option<Now>) -> Self {
        Self {
            about,
            town,
            inbox: None,
            connections: 0,
            now,
            owed: HashMap::new(),
            told: BTreeSet::new(),
            speaking: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

/// One resident on record, as `fastctx w` needs it.
pub struct OnRecord {
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
        let mut state = State::default();
        let mut problems = Vec::new();
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
                let about = match records::read_about(&directory) {
                    Ok(about) => about,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        problems.push((directory.about(), error));
                        continue;
                    }
                };
                let now = records::read_now(&directory).unwrap_or_else(|error| {
                    problems.push((directory.now(), error));
                    None
                });
                if let Some(session) = &about.session {
                    state.threads.insert(session.clone(), about.id.clone());
                }
                let mut live = Live::new(about, town.clone(), now);
                // Who was owed a turn the node stopped under: it is told how
                // that turn ended once the node is up (`unfinished`).
                if let Some(now) = &live.now
                    && now.activity == Activity::Busy
                {
                    let turn = now.turn.clone().unwrap_or_default();
                    live.owed.insert(turn, now.owed.iter().cloned().collect());
                }
                state.live.insert(live.about.id.clone(), live);
            }
        }
        Ok((Self { state: Mutex::new(state) }, problems))
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
            live.connections += 1;
            return Ok(Whereabouts { resident, town: live.town.clone() });
        }
        if let Some(resident) = hints.was
            && let Some(live) = state.live.get_mut(&resident)
        {
            live.connections += 1;
            return Ok(Whereabouts { resident, town: live.town.clone() });
        }
        if let Some(inbox) = &hints.inbox
            && let Some(live) = state.live.values_mut().find(|live| live.inbox.as_ref() == Some(inbox))
        {
            live.connections += 1;
            return Ok(Whereabouts { resident: live.about.id.clone(), town: live.town.clone() });
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
        live.inbox = hints.inbox;
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
                if live.connections == 0 && !live.about.world_started {
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
        records::write_now(&directory, &now)?;
        let mut state = self.lock();
        if let Some(session) = &about.session {
            state.threads.insert(session.clone(), about.id.clone());
        }
        state.live.insert(about.id.clone(), Live::new(about, town, Some(now)));
        Ok(())
    }

    /// The resident whose Claude session has the inbox address `inbox`.
    pub fn resident_of_inbox(&self, inbox: &str) -> Option<ResidentId> {
        self.lock().live.values().find(|live| live.inbox.as_deref() == Some(inbox)).map(|live| live.about.id.clone())
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

    /// Words of `readers` went in, and the turn they go into is not known
    /// yet: they are owed the first turn that ends.
    pub fn owe_next(&self, id: &ResidentId, readers: &[Address]) {
        if let Some(live) = self.lock().live.get_mut(id) {
            live.owed.entry(String::new()).or_default().extend(readers.iter().cloned());
        }
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

    /// Every resident on record.
    pub fn everyone(&self) -> Vec<OnRecord> {
        let state = self.lock();
        state
            .live
            .iter()
            .map(|(id, live)| OnRecord {
                id: id.clone(),
                town: live.town.clone(),
                here: here(live),
                about: live.about.clone(),
                now: live.now.clone(),
            })
            .collect()
    }

    /// Residents whose session the World holds and that were working when
    /// the node stopped, with the turn they ran (empty when its id was not
    /// known yet).
    pub fn unfinished(&self) -> Vec<(ResidentId, String)> {
        self.lock()
            .live
            .values()
            .filter(|live| live.about.world_started)
            .filter_map(|live| {
                let now = live.now.as_ref()?;
                (now.activity == Activity::Busy).then(|| (live.about.id.clone(), now.turn.clone().unwrap_or_default()))
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
        let mut now = live.now.clone().unwrap_or_else(|| idle(Timestamp::now()));
        change(&mut now);
        records::write_now(&world.resident(live.town.as_ref(), id), &now)?;
        live.now = Some(now);
        Ok(())
    }

    /// The session runs `turn`: what was told about an earlier one is not
    /// told about it.
    pub fn turn_started(&self, world: &WorldDir, id: &ResidentId, turn: &str) -> io::Result<()> {
        self.with_live(world, id, |live, now| {
            if now.turn.as_deref() != Some(turn) {
                live.told.clear();
            }
            enter(now, turn);
        })
    }

    /// Words of `readers` went into `turn`: each is owed its end.
    pub fn owe(&self, world: &WorldDir, id: &ResidentId, turn: &str, readers: &[Address]) -> io::Result<()> {
        self.with_live(world, id, |live, now| {
            if now.turn.as_deref() != Some(turn) {
                live.told.clear();
            }
            live.owed.entry(turn.to_string()).or_default().extend(readers.iter().cloned());
            enter(now, turn);
            now.owed = owed(live);
        })
    }

    /// Who was owed `turn`'s end, taken now that it ended; the resident is
    /// idle, or asleep when whoever it works for is gone, unless words
    /// meanwhile started another turn. `outage` says the turn did not end
    /// on its own.
    pub fn take_owed(
        &self,
        world: &WorldDir,
        id: &ResidentId,
        turn: &str,
        requester_here: bool,
        trouble: Option<Trouble>,
    ) -> io::Result<Vec<Address>> {
        let mut readers = Vec::new();
        self.with_live(world, id, |live, now| {
            // Words whose turn was not known when they went in are owed the
            // first turn that ends.
            let mut taken = live.owed.remove(turn).unwrap_or_default();
            taken.extend(live.owed.remove("").unwrap_or_default());
            readers = taken.into_iter().collect();
            // Words may meanwhile have started the next turn, whose state
            // stands.
            if now.turn.as_deref() == Some(turn) || now.turn.is_none() {
                now.turn = None;
                now.activity = if requester_here { Activity::Idle } else { Activity::Asleep };
                now.since = Timestamp::now();
                live.told.clear();
            }
            if !readers.is_empty() {
                now.answering = readers.clone();
            }
            now.owed = owed(live);
            now.trouble = trouble;
        })?;
        Ok(readers)
    }

    /// Whether an urgent kind of health is told now: the first of its kind
    /// in the running turn. Returns who is owed the turn's end then.
    pub fn first_told(&self, id: &ResidentId, kind: HealthKind) -> Vec<Address> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Vec::new();
        };
        if live.told.insert(kind) { owed(live) } else { Vec::new() }
    }

    fn with_live(&self, world: &WorldDir, id: &ResidentId, change: impl FnOnce(&mut Live, &mut Now)) -> io::Result<()> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Ok(());
        };
        let mut now = live.now.clone().unwrap_or_else(|| idle(Timestamp::now()));
        change(live, &mut now);
        records::write_now(&world.resident(live.town.as_ref(), id), &now)?;
        live.now = Some(now);
        Ok(())
    }
}

fn here(live: &Live) -> bool {
    if live.about.world_started {
        live.now.as_ref().is_none_or(|now| now.activity != Activity::Asleep)
    } else {
        live.connections > 0
    }
}

fn enter(now: &mut Now, turn: &str) {
    if now.turn.as_deref() != Some(turn) || now.activity != Activity::Busy {
        now.since = Timestamp::now();
    }
    now.turn = Some(turn.to_string());
    now.activity = Activity::Busy;
}

fn owed(live: &Live) -> Vec<Address> {
    live.owed.values().flatten().cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

pub fn idle(since: Timestamp) -> Now {
    Now { activity: Activity::Idle, since, turn: None, owed: Vec::new(), answering: Vec::new(), trouble: None }
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

/// The town a directory of residents belongs to, from where it lies.
fn town_of(world: &WorldDir, parent: &std::path::Path) -> Option<TownName> {
    let towns = world.path().join("towns");
    let town = parent.parent()?.strip_prefix(&towns).ok()?;
    TownName::try_from(town.to_str()?).ok()
}
