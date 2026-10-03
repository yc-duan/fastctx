//! The Towns page and the chat screen: who lives in the World, on every machine, and a live
//! conversation with one of them (`specs/world-town.md`, `world-colleagues.md`).
//!
//! The page asks what `fastctx w` asks, through this machine's node, on a background thread,
//! again every few seconds while it is on screen; without the node it reads this machine's
//! records alone and says so. The chat screen holds one conversation; leaving it ends the
//! watch, never the resident.

use super::talk::{TalkClient, Target};
use crate::control::paths::ControlPaths;
use crate::town::w::Silent;
use crate::town::who::{self, JobFacts, MachineAnswer, Query, ResidentFacts};
use crate::tui::text_input::{InputMode, TextInput};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the census is read again while it is on screen. The page shows durations to the
/// second and a resident's state changes at its turn boundaries; every two seconds keeps it
/// current without reading every record each frame.
const CENSUS_EVERY: Duration = Duration::from_secs(2);

/// What one message may hold: the request carrying it is one admin frame, JSON escapes a
/// character into at most 12 bytes (a surrogate pair), and the rest of the request takes far
/// less than the 64 KiB kept for it.
const MESSAGE_LIMIT: usize = (crate::world::node::admin::MAX_ADMIN_FRAME - 64 * 1024) / 12;

/// The agents the World can open on this machine.
pub(super) const AGENTS: [&str; 2] = ["codex", "claude"];

/// Who lives in the World, as every machine that answered says.
pub(super) struct Census {
    /// This machine.
    pub(super) machine: String,
    /// The machines that answered, this one first.
    pub(super) machines: Vec<String>,
    /// The other machines that gave no answer, and why.
    pub(super) silent: Vec<Silent>,
    /// This machine's node is not running, so only its own records were read.
    pub(super) alone: bool,
    /// Residents not gone, grouped by town (the outside last), busiest first within a town.
    pub(super) residents: Vec<ResidentFacts>,
    /// Commands still running, by whom they were started.
    pub(super) running: Vec<JobFacts>,
    /// Every town on this machine, for opening a resident in one.
    pub(super) towns: Vec<TownName>,
    /// The selected resident with its recent actions and messages.
    pub(super) detail: Option<ResidentFacts>,
}

pub(super) struct TownName {
    pub(super) slug: String,
    pub(super) name: String,
    /// The machines its charter names: a resident can be opened in it on these.
    pub(super) machines: Vec<String>,
}

pub(super) enum CensusError {
    NotEnrolled,
    Failed(String),
}

/// A new resident before its first words: which agent, on which machine, in which town.
pub(super) struct Compose {
    pub(super) agent: usize,
    /// An index into the census's machines, which lists this machine first.
    pub(super) machine: usize,
    /// 0 is outside every town; `n` is town `n - 1` of those that reach the chosen machine.
    pub(super) town: usize,
    /// The row the arrow keys change; on `Words`, keys type the first words.
    pub(super) focus: Field,
}

/// A row of the new-resident screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Field {
    Agent,
    Machine,
    Town,
    Words,
}

impl Field {
    const ORDER: [Field; 4] = [Field::Agent, Field::Machine, Field::Town, Field::Words];

    fn step(self, forward: bool) -> Self {
        let at = Self::ORDER.iter().position(|field| *field == self).unwrap_or(0);
        let next = if forward { at + 1 } else { at + Self::ORDER.len() - 1 };
        Self::ORDER[next % Self::ORDER.len()]
    }
}

impl Census {
    /// The towns a resident can be opened in on `machine`.
    pub(super) fn towns_on(&self, machine: &str) -> Vec<&TownName> {
        self.towns.iter().filter(|town| town.machines.iter().any(|name| name == machine)).collect()
    }
}

pub(super) struct Chat {
    /// The conversation; `None` while a new resident is being composed.
    pub(super) client: Option<TalkClient>,
    pub(super) input: TextInput,
    pub(super) compose: Option<Compose>,
    /// Rows scrolled back from the newest; 0 follows the conversation as it grows.
    pub(super) back: usize,
    /// Set while drawing: how far back the transcript goes, and how many rows a page is.
    pub(super) max_back: usize,
    pub(super) page_rows: usize,
    /// When the running turn started, as this screen saw it.
    pub(super) busy_since: Option<Instant>,
}

/// What a key did on the chat screen.
pub(super) enum ChatKey {
    Stay,
    Leave,
}

#[derive(Default)]
pub(super) struct Towns {
    pub(super) census: Option<Result<Census, CensusError>>,
    reader: Option<mpsc::Receiver<Result<Census, CensusError>>>,
    last_read: Option<Instant>,
    /// The resident whose details the last read was asked for.
    asked_detail: Option<String>,
    pub(super) chat: Option<Chat>,
}

impl Towns {
    /// Takes in a finished read and starts the next one when it is due; true when anything on
    /// screen changed.
    pub(super) fn tick(&mut self, paths: &ControlPaths, visible: bool, selected: Option<String>) -> bool {
        let mut changed = false;
        if let Some(reader) = &self.reader
            && let Ok(census) = reader.try_recv()
        {
            self.reader = None;
            self.census = Some(census);
            changed = true;
        }
        if let Some(chat) = &mut self.chat
            && let Some(client) = &mut chat.client
            && client.poll()
        {
            changed = true;
            match (client.transcript.busy, chat.busy_since) {
                (true, None) => chat.busy_since = Some(Instant::now()),
                (false, Some(_)) => chat.busy_since = None,
                _ => {}
            }
        }
        let due = self.last_read.is_none_or(|last| last.elapsed() >= CENSUS_EVERY) || selected != self.asked_detail;
        if visible && self.reader.is_none() && due {
            let (sender, receiver) = mpsc::channel();
            let paths = paths.clone();
            let detail = selected.clone();
            let spawned = std::thread::Builder::new().name("fastctx-world-towns".into()).spawn(move || {
                let _ = sender.send(read(&paths, detail));
            });
            match spawned {
                Ok(_) => self.reader = Some(receiver),
                Err(error) => self.census = Some(Err(CensusError::Failed(error.to_string()))),
            }
            self.last_read = Some(Instant::now());
            self.asked_detail = selected;
        }
        changed
    }

    pub(super) fn residents(&self) -> &[ResidentFacts] {
        match &self.census {
            Some(Ok(census)) => &census.residents,
            _ => &[],
        }
    }

    pub(super) fn census(&self) -> Option<&Census> {
        self.census.as_ref().and_then(|census| census.as_ref().ok())
    }

    /// Opens the chat screen on a resident.
    pub(super) fn talk_to(&mut self, resident: &str) {
        self.chat = Some(Chat::new(Some(TalkClient::open(Target::Resident(resident.to_string()))), None));
    }

    /// Opens the chat screen on a resident yet to be opened; its first words open it.
    pub(super) fn compose(&mut self) {
        self.chat = Some(Chat::new(None, Some(Compose { agent: 0, machine: 0, town: 0, focus: Field::Words })));
    }

    /// Stops the running turn of a resident from the page, without opening a conversation.
    pub(super) fn stop(&mut self, resident: &str) {
        let resident = resident.to_string();
        let _ = std::thread::Builder::new().name("fastctx-world-stop".into()).spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                let Ok(environment) = crate::session::SessionEnvironment::capture() else { return };
                let Ok(endpoint) = crate::runtime::node_admin_endpoint(&environment) else { return };
                let request = crate::world::node::admin::AdminRequest::Town {
                    request: crate::town::admin::TownRequest::Interrupt { resident },
                };
                let _ = crate::world::node::admin::call(&endpoint, &request, crate::world::node::admin::ADMIN_TIMEOUT).await;
            });
        });
    }

    pub(super) fn paste(&mut self, text: &str) {
        if let Some(chat) = &mut self.chat {
            chat.input.insert(text);
        }
    }

    /// Handles a key on the chat screen.
    pub(super) fn chat_key(&mut self, key: KeyEvent) -> ChatKey {
        if key.code == KeyCode::Esc {
            self.chat = None;
            return ChatKey::Leave;
        }
        // What the new-resident rows choose from: the machines that answered, this one first,
        // and for each the towns that reach it.
        let (machines, towns): (Vec<String>, Vec<Vec<String>>) = match self.census() {
            Some(census) => (
                census.machines.clone(),
                census.machines.iter().map(|machine| census.towns_on(machine).iter().map(|town| town.slug.clone()).collect()).collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let Some(chat) = &mut self.chat else { return ChatKey::Leave };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(compose) = &mut chat.compose
            && compose.choose(key, machines.len(), &towns)
        {
            return ChatKey::Stay;
        }
        match key.code {
            KeyCode::Enter if !control => chat.submit(&machines, &towns),
            KeyCode::Char('x') if control => {
                if let Some(client) = &mut chat.client
                    && client.transcript.busy
                {
                    client.stop();
                }
            }
            KeyCode::PageUp => chat.back = chat.back.saturating_add(chat.page_rows.max(1)).min(chat.max_back),
            KeyCode::PageDown => chat.back = chat.back.saturating_sub(chat.page_rows.max(1)),
            KeyCode::Up if chat.input.text().is_empty() => chat.back = chat.back.saturating_add(1).min(chat.max_back),
            KeyCode::Down if chat.input.text().is_empty() => chat.back = chat.back.saturating_sub(1),
            _ => {
                chat.input.handle_key(key);
            }
        }
        ChatKey::Stay
    }

    /// The resident a chat is with, once known.
    pub(super) fn chat_resident(&self) -> Option<&str> {
        self.chat.as_ref()?.client.as_ref()?.resident.as_deref()
    }

    /// The census facts of the resident a chat is with.
    pub(super) fn chat_facts(&self) -> Option<&ResidentFacts> {
        let id = self.chat_resident()?;
        self.residents().iter().find(|facts| facts.about.id == id)
    }
}

impl Chat {
    pub(super) fn new(client: Option<TalkClient>, compose: Option<Compose>) -> Self {
        Self {
            client,
            input: TextInput::new(String::new(), MESSAGE_LIMIT, InputMode::Multiline, false),
            compose,
            back: 0,
            max_back: 0,
            page_rows: 1,
            busy_since: None,
        }
    }

    /// Says what was typed, or opens the new resident with it as its first words.
    fn submit(&mut self, machines: &[String], towns: &[Vec<String>]) {
        let text = self.input.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(compose) = self.compose.take() {
            let town = compose.town.checked_sub(1).and_then(|index| towns.get(compose.machine)?.get(index)).cloned();
            // The census lists this machine first; any other is where the resident opens.
            let node = (compose.machine > 0).then(|| machines.get(compose.machine).cloned()).flatten();
            self.client = Some(TalkClient::open(Target::New { agent: AGENTS[compose.agent].to_string(), town, node, words: text }));
        } else if let Some(client) = &mut self.client {
            client.say(text);
        } else {
            return;
        }
        self.input.replace(String::new());
        self.back = 0;
    }
}

impl Compose {
    /// Moves between the rows or changes the focused one; `false` when the key is not for
    /// them, and a printable key on a row starts typing the first words.
    fn choose(&mut self, key: KeyEvent, machines: usize, towns: &[Vec<String>]) -> bool {
        match key.code {
            KeyCode::Tab => self.focus = self.focus.step(true),
            KeyCode::BackTab => self.focus = self.focus.step(false),
            KeyCode::Left | KeyCode::Right if self.focus != Field::Words => {
                let forward = key.code == KeyCode::Right;
                let cycle = |at: usize, count: usize| if count == 0 { 0 } else if forward { (at + 1) % count } else { (at + count - 1) % count };
                match self.focus {
                    Field::Agent => self.agent = cycle(self.agent, AGENTS.len()),
                    Field::Machine => {
                        self.machine = cycle(self.machine, machines.max(1));
                        self.town = 0;
                    }
                    Field::Town => self.town = cycle(self.town, towns.get(self.machine).map_or(0, Vec::len) + 1),
                    Field::Words => {}
                }
            }
            KeyCode::Char(_) if self.focus != Field::Words && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Field::Words;
                return false;
            }
            _ => return false,
        }
        true
    }
}

/// Reads who lives in the World: every machine's answer as this machine's node gathers it, or
/// this machine's own records when its node is not running.
fn read(paths: &ControlPaths, detail: Option<String>) -> Result<Census, CensusError> {
    let world = crate::world::WorldPaths::from_control(paths);
    let config = crate::world::load_config(&world).map_err(CensusError::Failed)?.ok_or(CensusError::NotEnrolled)?;
    let machine = config.name;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error| CensusError::Failed(error.to_string()))?;
    let ask = |query: Query| -> Result<(Vec<MachineAnswer>, Vec<Silent>, bool), CensusError> {
        match runtime.block_on(crate::town::cli::gather(&query)).map_err(CensusError::Failed)? {
            Some((answers, silent)) => Ok((answers, silent, false)),
            None => Ok((vec![who::answer(&world.dir, &machine, &query)], Vec::new(), true)),
        }
    };
    let (answers, silent, alone) = ask(Query { all: true, everywhere: true, ..Query::default() })?;
    let machines: Vec<String> = answers.iter().map(|answer| answer.machine.clone()).collect();
    let every: Vec<ResidentFacts> = answers.iter().flat_map(|answer| answer.residents.iter().cloned()).collect();
    let running: Vec<JobFacts> = answers.into_iter().flat_map(|answer| answer.jobs).filter(JobFacts::running).collect();
    // A resident that moved between towns left records in each; the current ones count. Ids
    // are each machine's own, so a resident is its machine and its id.
    let mut keys: Vec<(String, String)> = every.iter().map(|facts| (facts.about.machine.clone(), facts.about.id.clone())).collect();
    keys.sort();
    keys.dedup();
    let mut residents: Vec<ResidentFacts> = keys
        .iter()
        .filter_map(|(home, id)| {
            who::current(every.iter().filter(|facts| facts.about.machine == *home && facts.about.id == *id).collect()).cloned()
        })
        .filter(|facts| facts.now.state != "left")
        .collect();
    let rank = |state: &str| match state {
        "busy" => 0,
        "waiting" => 1,
        "idle" | "unknown" => 2,
        _ => 3,
    };
    residents.sort_by(|left, right| {
        (left.about.town.is_none(), &left.about.town, rank(&left.now.state), std::cmp::Reverse(activity(left))).cmp(&(
            right.about.town.is_none(),
            &right.about.town,
            rank(&right.now.state),
            std::cmp::Reverse(activity(right)),
        ))
    });
    let towns = crate::town::project::towns(&world.dir)
        .into_iter()
        .map(|slug| match crate::town::project::charter(&world.dir, &slug) {
            Ok(charter) => TownName { slug, name: charter.name, machines: charter.machines },
            Err(_) => TownName { name: slug.clone(), slug, machines: Vec::new() },
        })
        .collect();
    let detail = match detail {
        Some(id) => {
            let (answers, _, _) = ask(Query { ids: vec![id], ..Query::default() })?;
            let found: Vec<&ResidentFacts> = answers.iter().flat_map(|answer| &answer.residents).collect();
            who::current(found).cloned()
        }
        None => None,
    };
    Ok(Census { machine, machines, silent, alone, residents, running, towns, detail })
}

/// When a resident last did anything the records show.
fn activity(facts: &ResidentFacts) -> String {
    facts.now.last_action_at.clone().unwrap_or_default().max(facts.now.since.clone())
}
