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
//! Who is owed the end of each running turn is kept in memory and written
//! through to its `now` on every change, so a node that restarts knows whom
//! to tell.
//!
//! A resident has a directory in each town it lived in; all but the one it
//! lives in now are marked as left (`records::move_to`).

mod records;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{About, Activity, Hints, Host, Moved, Now, Origin, Trouble, Whereabouts};
use fastctx_types::session::HealthKind;
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

pub use records::{append_message, last_action, read_about, record_action, record_seen};

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
    about: About,
    town: Option<TownName>,
    /// A Claude session's inbox address, from its MCP connection.
    inbox: Option<String>,
    /// Open connections the resident arrived on.
    connections: usize,
    now: Option<Now>,
    /// For each turn running, whose words went into it.
    owed: HashMap<String, BTreeSet<Address>>,
    /// The urgent kinds of health already told to those owed the running
    /// turn's end: each is told once a turn.
    told: BTreeSet<HealthKind>,
    /// Held while words go into its session until the turn they joined is
    /// known and owed, and while a turn's end is taken: so a turn's end goes
    /// to exactly those whose words went into it.
    speaking: Arc<tokio::sync::Mutex<()>>,
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
            about,
            town,
            inbox: None,
            connections: 0,
            now,
            owed,
            told: BTreeSet::new(),
            speaking: Arc::new(tokio::sync::Mutex::new(())),
            heard: None,
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
            let now = records::read_now(&directory).unwrap_or_else(|error| {
                problems.push((directory.now(), error));
                None
            });
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
            let live = Live::new(about, town.clone(), now);
            state.live.insert(live.about.id.clone(), live);
        }
        Ok((Self { state: Mutex::new(state) }, problems))
    }

    /// The resident `id` lives in `town` from now on. Returns whether it
    /// moved: it may live there already.
    pub fn enter(&self, world: &WorldDir, id: &ResidentId, town: &TownName) -> io::Result<bool> {
        let mut state = self.lock();
        let Some(live) = state.live.get_mut(id) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("no resident {id} lives here")));
        };
        if live.town.as_ref() == Some(town) {
            return Ok(false);
        }
        let from = live.town.clone();
        records::move_to(world, id, from.as_ref(), Some(town), &live.about, live.now.as_ref())?;
        live.town = Some(town.clone());
        state.former.retain(|former| !(former.id == *id && former.town.as_ref() == Some(town)));
        state.former.push(Former { id: id.clone(), town: from, to: Some(town.clone()), at: Timestamp::now() });
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
                    if hints.inbox.is_some() {
                        live.inbox = hints.inbox;
                    }
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
            // A Claude session arriving again after the node restarted pairs
            // by this address again.
            if hints.inbox.is_some() {
                live.inbox = hints.inbox;
            }
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

    /// Codex residents the World holds that were working when the node
    /// stopped, with the turn they ran (empty when none had started): their
    /// app-server stopped with the node. A Claude session runs on under
    /// Claude Code's own supervisor, and its turn ends as it ends.
    pub fn unfinished(&self) -> Vec<(ResidentId, String)> {
        self.lock()
            .live
            .values()
            .filter(|live| live.about.world_started && live.about.host.agent.as_str() == "codex")
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
        match now.turn.as_ref().filter(|_| now.activity == Activity::Busy).and_then(|turn| live.owed.get(turn)) {
            Some(readers) if !readers.is_empty() => readers.iter().cloned().collect(),
            _ => now.answering.clone(),
        }
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
        self.with_live(world, id, |live, now| {
            readers = live.owed.remove(turn).unwrap_or_default().into_iter().collect();
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
        if live.told.insert(kind) { everyone_owed(live) } else { Vec::new() }
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

/// Who is owed each running turn, as `now` keeps it.
fn owed(live: &Live) -> BTreeMap<String, Vec<Address>> {
    live.owed
        .iter()
        .filter(|(_, readers)| !readers.is_empty())
        .map(|(turn, readers)| (turn.clone(), readers.iter().cloned().collect()))
        .collect()
}

/// Everyone owed any running turn, once each.
fn everyone_owed(live: &Live) -> Vec<Address> {
    live.owed.values().flatten().cloned().collect::<BTreeSet<_>>().into_iter().collect()
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
