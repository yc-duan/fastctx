//! The TUI's state: a stack of pages, the top one on the screen. A page
//! answers keys with what should happen next; work for the World runs
//! beside the screen, and its answer comes back to the page that asked,
//! wherever that page is in the stack by then.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_client::{Said, TownRecord, TownView};
use fastctx_types::project::{JournalView, MapView, RecordFields, TownSummary};
use fastctx_types::session::SessionEvent;
use fastctx_types::town::{ResidentNow, RunningJob};
use fastctx_types::world::NodesReply;
use fastctx_types::{AgentName, EntryId, JournalName, MachineName, ResidentId, TownName};
use jiff::Timestamp;
use ratatui::Frame;

use crate::design::Look;
use crate::input::Input;
use crate::pages::Page;
use crate::sessions::{Report, Status};
use crate::words::Words;

/// What every page draws with.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub look: Look,
    pub words: Words,
    pub now: Timestamp,
    /// How far a mark of something happening has faded, 0 to 1: it
    /// breathes while something is busy.
    pub breath: f32,
    /// Tenths of a second since the screen opened: what turns, turns by it.
    pub frame: u64,
}

/// What a page wants after a key or an answer.
pub enum Act {
    Stay,
    Push(Page),
    /// This page gives way to another, as a form does to what it made.
    Replace(Page),
    Back,
    Quit,
    Do(Job),
}

/// Something to ask of the World for a page.
#[derive(Clone, Debug)]
pub enum Job {
    Talk {
        to: ResidentId,
        machine: MachineName,
        text: String,
    },
    Stop {
        resident: ResidentId,
        machine: MachineName,
    },
    Open {
        agent: AgentName,
        town: Option<TownName>,
        node: Option<MachineName>,
        text: String,
    },
    OpenTown {
        town: TownName,
        fields: RecordFields,
    },
    EditTown {
        town: TownName,
        fields: RecordFields,
    },
    Journal {
        town: TownName,
        journal: JournalName,
        before: Option<EntryId>,
    },
    Invite {
        name: Option<MachineName>,
        hours: u32,
    },
    Join {
        code: String,
        name: Option<MachineName>,
    },
    Revoke {
        name: MachineName,
        reason: String,
    },
    Leave,
    /// Put the World's tools into the user's own sessions on this machine;
    /// `raise` agrees to raising Codex's shared output limit.
    Apply {
        raise: bool,
    },
    /// Take them out again.
    Unapply,
}

/// What the World answered a job.
#[derive(Debug)]
pub enum Done {
    Said(Said),
    Stopped(bool),
    Opened(Said),
    Record(Box<TownRecord>),
    Journal(Box<JournalView>),
    Code {
        code: String,
        expires: Timestamp,
    },
    Joined {
        world: String,
        name: MachineName,
    },
    Revoked,
    Left,
    Applied(Report),
    Unapplied(Report),
    /// Codex's shared output limit is lower than the World needs, and
    /// raising it was not yet agreed to.
    NeedsRaise {
        current: i64,
        requested: u64,
    },
}

/// What a page keeps up to date with while it is on top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feed {
    World {
        all: bool,
    },
    Town {
        town: TownName,
        all: bool,
    },
    Nodes,
    /// The machines and projects to choose from when opening a session.
    Choices,
    /// A watch of a session; `round` counts the times it was started, so
    /// asking again starts it anew.
    Watch {
        resident: ResidentId,
        machine: MachineName,
        /// Its town, whose machines say what it started that still runs;
        /// outside a town, every machine of the World says.
        town: Option<TownName>,
        round: u32,
    },
}

/// Something that came back for a page.
#[derive(Debug)]
pub enum Data {
    World {
        view: Result<TownView, String>,
        nodes: Result<NodesReply, String>,
    },
    Town {
        view: Result<TownView, String>,
        map: Result<Box<MapView>, String>,
    },
    Nodes(Result<NodesReply, String>),
    Choices {
        nodes: Result<NodesReply, String>,
        towns: Result<Vec<TownSummary>, String>,
    },
    History(Vec<SessionEvent>),
    Event(SessionEvent),
    /// The watch ended, with why when it broke.
    Unwatched(Option<String>),
    /// What the agent being watched has going on besides its turns.
    Around(Result<Around, String>),
    /// Whether the user's own sessions here have the World's tools.
    Sessions(Result<Status, String>),
    /// What the World answered a job the page asked for.
    Done {
        job: Job,
        result: Result<Done, String>,
    },
}

/// What an agent has going on besides its turns, wherever it is.
#[derive(Clone, Debug, Default)]
pub struct Around {
    /// What it started that still runs.
    pub jobs: Vec<RunningJob>,
    /// The colleagues it asked for.
    pub colleagues: Vec<ResidentNow>,
}

pub struct App {
    pub ctx: Ctx,
    pages: Vec<(u64, Page)>,
    next_id: u64,
    pub quit: bool,
    started: Instant,
    /// A paste is arriving: this many characters so far.
    pasting: Option<usize>,
}

impl App {
    pub fn new(look: Look, words: Words, root: Page) -> Self {
        let ctx = Ctx { look, words, now: Timestamp::now(), breath: 0.0, frame: 0 };
        Self { ctx, pages: vec![(0, root)], next_id: 1, quit: false, started: Instant::now(), pasting: None }
    }

    /// Advances the clock and the breathing of busy marks.
    pub fn tick(&mut self) {
        self.ctx.now = Timestamp::now();
        // One breath every 2.4 seconds: slow enough to stay quiet.
        let phase = (self.started.elapsed().as_millis() % 2400) as f32 / 2400.0;
        self.ctx.breath = 0.55 * (0.5 - 0.5 * (phase * std::f32::consts::TAU).cos());
        self.ctx.frame = (self.started.elapsed().as_millis() / 100) as u64;
    }

    fn top(&mut self) -> (u64, &mut Page) {
        let (id, page) = self.pages.last_mut().expect("the World page is never left");
        (*id, page)
    }

    /// What the page on top keeps up to date with.
    pub fn feed(&self) -> Option<(u64, Feed)> {
        let (id, page) = self.pages.last().expect("the World page is never left");
        page.feed().map(|feed| (*id, feed))
    }

    /// Whether anything on the screen is happening now, so it breathes.
    pub fn animating(&self) -> bool {
        self.pages.last().is_some_and(|(_, page)| page.animating()) || self.pasting.is_some()
    }

    pub fn input(&mut self, input: Input) -> Vec<(u64, Job)> {
        match input {
            Input::Key(key) if is_quit(&key) => {
                self.quit = true;
                Vec::new()
            }
            Input::Key(key) => {
                let ctx = self.ctx;
                let (id, page) = self.top();
                let act = page.key(&ctx, key);
                self.apply(id, act)
            }
            Input::Paste(text) => {
                self.pasting = None;
                let (id, page) = self.top();
                let act = page.paste(text);
                self.apply(id, act)
            }
            Input::Pasting(count) => {
                self.pasting = Some(count);
                Vec::new()
            }
            // A sequence no page reads, such as a terminal's answer nobody asked for.
            Input::Resize | Input::Sequence(_) => Vec::new(),
            Input::Failed(_) => {
                self.quit = true;
                Vec::new()
            }
        }
    }

    /// An answer for the page `id`; dropped when that page is gone.
    pub fn data(&mut self, id: u64, data: Data) -> Vec<(u64, Job)> {
        let ctx = self.ctx;
        let Some(page) = self.pages.iter_mut().find(|(page_id, _)| *page_id == id).map(|(_, page)| page) else {
            return Vec::new();
        };
        let act = page.data(&ctx, data);
        self.apply(id, act)
    }

    fn apply(&mut self, id: u64, act: Act) -> Vec<(u64, Job)> {
        match act {
            Act::Stay => Vec::new(),
            Act::Push(mut page) => {
                let new = self.next_id;
                self.next_id += 1;
                let start = page.start();
                self.pages.push((new, page));
                start.map(|job| vec![(new, job)]).unwrap_or_default()
            }
            Act::Replace(mut page) => {
                let Some(at) = self.pages.iter().position(|(page_id, _)| *page_id == id).filter(|at| *at > 0) else {
                    return Vec::new();
                };
                let new = self.next_id;
                self.next_id += 1;
                let start = page.start();
                self.pages[at] = (new, page);
                start.map(|job| vec![(new, job)]).unwrap_or_default()
            }
            Act::Back => {
                if self.pages.len() > 1 && self.pages.last().is_some_and(|(page_id, _)| *page_id == id) {
                    self.pages.pop();
                }
                Vec::new()
            }
            Act::Quit => {
                self.quit = true;
                Vec::new()
            }
            Act::Do(job) => vec![(id, job)],
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let ctx = self.ctx;
        let area = frame.area();
        let pasting = self.pasting;
        let (_, page) = self.top();
        page.draw(&ctx, frame, area, pasting);
    }
}

/// Ctrl+C leaves from anywhere.
fn is_quit(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}
