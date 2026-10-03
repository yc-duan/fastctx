//! The Towns page and the chat screen: who lives in the World on this machine, and a live
//! conversation with one of them (`specs/world-town.md`, `world-colleagues.md`).
//!
//! The page reads the same records `fastctx w` reads, on a background thread, again every few
//! seconds while it is on screen. The chat screen holds one conversation; leaving it ends the
//! watch, never the resident.

use super::talk::{TalkClient, Target};
use crate::control::paths::ControlPaths;
use crate::town::who::{self, JobFacts, Query, ResidentFacts};
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

/// Who lives in the World on this machine, as the records say.
pub(super) struct Census {
    pub(super) machine: String,
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
}

pub(super) enum CensusError {
    NotEnrolled,
    Failed(String),
}

/// A new resident before its first words: which agent, in which town.
pub(super) struct Compose {
    pub(super) agent: usize,
    /// 0 is outside every town; `n` is the census's town `n - 1`.
    pub(super) town: usize,
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
        self.chat = Some(Chat::new(None, Some(Compose { agent: 0, town: 0 })));
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
        let town_names: Vec<String> = self.census().map(|census| census.towns.iter().map(|town| town.slug.clone()).collect()).unwrap_or_default();
        let towns = town_names.len();
        let Some(chat) = &mut self.chat else { return ChatKey::Leave };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter if !control => chat.submit(&town_names),
            KeyCode::Char('x') if control => {
                if let Some(client) = &mut chat.client
                    && client.transcript.busy
                {
                    client.stop();
                }
            }
            KeyCode::Tab if chat.compose.is_some() => {
                if let Some(compose) = &mut chat.compose {
                    compose.agent = (compose.agent + 1) % AGENTS.len();
                }
            }
            KeyCode::BackTab if chat.compose.is_some() => {
                if let Some(compose) = &mut chat.compose {
                    compose.town = (compose.town + 1) % (towns + 1);
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
    fn submit(&mut self, towns: &[String]) {
        let text = self.input.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(compose) = self.compose.take() {
            let town = compose.town.checked_sub(1).and_then(|index| towns.get(index)).cloned();
            self.client = Some(TalkClient::open(Target::New { agent: AGENTS[compose.agent].to_string(), town, words: text }));
        } else if let Some(client) = &mut self.client {
            client.say(text);
        } else {
            return;
        }
        self.input.replace(String::new());
        self.back = 0;
    }
}

/// Reads who lives here from the records under this machine's World directory.
fn read(paths: &ControlPaths, detail: Option<String>) -> Result<Census, CensusError> {
    let world = crate::world::WorldPaths::from_control(paths);
    let config = crate::world::load_config(&world).map_err(CensusError::Failed)?.ok_or(CensusError::NotEnrolled)?;
    let machine = config.name;
    let slugs = crate::town::project::towns(&world.dir);
    let mut every = Vec::new();
    let mut running = Vec::new();
    for town in std::iter::once(None).chain(slugs.iter().cloned().map(Some)) {
        let answer = who::answer(&world.dir, &machine, &Query { town, all: true, ids: Vec::new() });
        every.extend(answer.residents);
        running.extend(answer.jobs);
    }
    // A resident that moved between towns left records in each; the current ones count.
    let mut ids: Vec<String> = every.iter().map(|facts| facts.about.id.clone()).collect();
    ids.sort();
    ids.dedup();
    let mut residents: Vec<ResidentFacts> = ids
        .iter()
        .filter_map(|id| who::current(every.iter().filter(|facts| &facts.about.id == id).collect()).cloned())
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
    let towns = slugs
        .into_iter()
        .map(|slug| {
            let name = crate::town::project::charter(&world.dir, &slug).map(|charter| charter.name).unwrap_or_else(|_| slug.clone());
            TownName { slug, name }
        })
        .collect();
    let detail = detail.and_then(|id| {
        let answer = who::answer(&world.dir, &machine, &Query { ids: vec![id], ..Query::default() });
        who::current(answer.residents.iter().collect()).cloned()
    });
    Ok(Census { machine, residents, running, towns, detail })
}

/// When a resident last did anything the records show.
fn activity(facts: &ResidentFacts) -> String {
    facts.now.last_action_at.clone().unwrap_or_default().max(facts.now.since.clone())
}
