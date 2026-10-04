//! Who is on this machine: finding a session's resident, or making one, and
//! knowing who is here.
//!
//! A resident the World started is named in its environment. A Codex
//! session is recognised by its thread, which stays the same across the
//! several MCP connections Codex may open for it. Any other session is a
//! resident of its own from its first call. A resident is here while the
//! connection its session arrived on stays open: the session holds it for
//! as long as it lives, and when it closes the resident has left.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;

use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::{About, Action, Hints, Host, Origin, Whereabouts};
use fastctx_types::{MachineName, ResidentId, TownName};
use jiff::Timestamp;

pub struct Residents {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    known: HashMap<ResidentId, Option<TownName>>,
    threads: HashMap<String, ResidentId>,
    /// How many open connections each resident arrived on.
    here: HashMap<ResidentId, usize>,
}

impl Residents {
    /// Reads every resident this machine has a record of. A record that
    /// cannot be read is reported and left as it is.
    pub fn load(world: &WorldDir) -> io::Result<(Self, Vec<(std::path::PathBuf, io::Error)>)> {
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
                let path = entry?.path().join("about");
                let about: About = match std::fs::read(&path) {
                    Ok(bytes) => match serde_json::from_slice(&bytes) {
                        Ok(about) => about,
                        Err(error) => {
                            problems.push((path, io::Error::new(io::ErrorKind::InvalidData, error)));
                            continue;
                        }
                    },
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        problems.push((path, error));
                        continue;
                    }
                };
                if let Some(session) = &about.session {
                    state.threads.insert(session.clone(), about.id.clone());
                }
                state.known.insert(about.id, town.clone());
            }
        }
        Ok((Self { state: Mutex::new(state) }, problems))
    }

    /// The resident a session is, made now if it is new.
    pub fn arrive(&self, world: &WorldDir, machine: &MachineName, host: Host, hints: Hints) -> Result<Whereabouts, Failure> {
        let mut state = self.state.lock().expect("never poisoned");
        if let Some(resident) = hints.resident {
            return match state.known.get(&resident).cloned() {
                Some(town) => {
                    *state.here.entry(resident.clone()).or_default() += 1;
                    Ok(Whereabouts { resident, town })
                }
                None => Err(Failure::new(format!(
                    "this session says it is {resident}, but {machine} has no record of that resident"
                ))),
            };
        }
        if let Some(thread) = &hints.thread
            && let Some(resident) = state.threads.get(thread).cloned()
        {
            let town = state.known.get(&resident).cloned().flatten();
            *state.here.entry(resident.clone()).or_default() += 1;
            return Ok(Whereabouts { resident, town });
        }
        let resident = new_id(&host, &state)?;
        let about = About {
            id: resident.clone(),
            host,
            machine: machine.clone(),
            born_at: Timestamp::now(),
            origin: Origin::User,
            session: hints.thread.clone(),
        };
        let directory = world.resident(None, &resident);
        let written = ensure_private_dir(directory.path()).and_then(|()| {
            let mut bytes = serde_json::to_vec_pretty(&about).map_err(io::Error::other)?;
            bytes.push(b'\n');
            publish_new(&directory.about(), &bytes, Permissions::Private)
        });
        written.map_err(|error| Failure::new(format!("recording a new resident: {error}")))?;
        if let Some(thread) = hints.thread {
            state.threads.insert(thread, resident.clone());
        }
        state.known.insert(resident.clone(), None);
        *state.here.entry(resident.clone()).or_default() += 1;
        Ok(Whereabouts { resident, town: None })
    }

    /// A connection that residents arrived on has closed.
    pub fn left(&self, residents: &[ResidentId]) {
        let mut state = self.state.lock().expect("never poisoned");
        for resident in residents {
            if let Some(count) = state.here.get_mut(resident) {
                *count -= 1;
                if *count == 0 {
                    state.here.remove(resident);
                }
            }
        }
    }

    /// Every resident on record, with its town and whether it is here.
    pub fn everyone(&self) -> Vec<(ResidentId, Option<TownName>, bool)> {
        let state = self.state.lock().expect("never poisoned");
        state.known.iter().map(|(id, town)| (id.clone(), town.clone(), state.here.contains_key(id))).collect()
    }
}

/// Appends one action to a resident's `actions`.
pub fn record_action(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>, action: &Action) -> io::Result<()> {
    use std::io::Write as _;
    let mut line = serde_json::to_vec(action).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(world.resident(town, resident).actions())?;
    file.write_all(&line)
}

/// A resident's latest action: the last line of its `actions`, read from
/// the end, so a record of any length costs the same.
pub fn last_action(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>) -> io::Result<Option<Action>> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let path = world.resident(town, resident).actions();
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let length = file.metadata()?.len();
    // Read back from the end until a whole last line is in hand.
    let mut tail = Vec::new();
    let mut from = length;
    loop {
        let step = from.min(4096);
        from -= step;
        file.seek(SeekFrom::Start(from))?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
        let body = tail.strip_suffix(b"\n").unwrap_or(&tail);
        if let Some(newline) = body.iter().rposition(|&b| b == b'\n') {
            return parse_action(&path, &body[newline + 1..]).map(Some);
        }
        if from == 0 {
            return if body.is_empty() { Ok(None) } else { parse_action(&path, body).map(Some) };
        }
    }
}

fn parse_action(path: &std::path::Path, line: &[u8]) -> io::Result<Action> {
    serde_json::from_slice(line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

/// Appends what a resident saw to its `seen` record, one line per file.
pub fn record_seen(world: &WorldDir, resident: &ResidentId, town: Option<&TownName>, seen: &[fastctx_types::residents::Seen]) -> io::Result<()> {
    use std::io::Write as _;
    let directory = world.resident(town, resident);
    let mut lines = Vec::new();
    for one in seen {
        serde_json::to_writer(&mut lines, one).map_err(io::Error::other)?;
        lines.push(b'\n');
    }
    // One write per call keeps each call's lines together.
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(directory.seen())?;
    file.write_all(&lines)
}

/// A fresh id for a resident of `host`: its agent name and four hex digits,
/// unused on this machine.
fn new_id(host: &Host, state: &State) -> Result<ResidentId, Failure> {
    loop {
        let mut bytes = [0u8; 2];
        getrandom::fill(&mut bytes).map_err(|error| Failure::new(format!("choosing a resident id: {error}")))?;
        let id = ResidentId::try_from(format!("{}-{:02x}{:02x}", host.agent, bytes[0], bytes[1]))
            .expect("an agent name and four hex digits");
        if !state.known.contains_key(&id) {
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
