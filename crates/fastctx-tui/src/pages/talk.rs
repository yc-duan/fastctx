//! Talking to one agent on any machine (`world-colleagues.md`, 用户在任何
//! 地方和居民说话): what came before in its session, then everything as it
//! happens, and a place to type below it. Words typed here go in as the
//! user's; the agent can be told to stop its turn.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_types::residents::{Activity, Origin};
use fastctx_types::session::Happened;
use fastctx_types::town::{ResidentNow, RunningJob};
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};

use super::{quiet, wrong};
use crate::app::{Act, Ctx, Data, Done, Feed, Job};
use crate::design::Role;
use crate::editor::{Edit, Editor};
use crate::paint;
use crate::transcript::Transcript;
use crate::{text, who};

/// The most rows the place to type grows to, as a share of the screen.
const EDITOR_SHARE: u16 = 3;

pub struct TalkPage {
    address: Address,
    agent: String,
    origin: Option<Origin>,
    model: Option<String>,
    project: Option<String>,
    town: Option<TownName>,
    /// What it started that still runs, wherever it runs.
    running: Result<Vec<RunningJob>, String>,
    /// What its host last reported it doing, and since when.
    state: Option<(Activity, Timestamp)>,
    transcript: Transcript,
    editor: Editor,
    /// The first row shown, when the view is not following the end.
    top: Option<usize>,
    watching: Watching,
    round: u32,
    /// What became of the latest ask to stop.
    stopping: bool,
    note: Option<Line<'static>>,
    /// The machine this TUI runs on, when known: words sent are said there.
    here: Option<MachineName>,
    /// Words sent and not answered yet, oldest first; the World answers
    /// each send in turn.
    pending: VecDeque<u64>,
    /// The conversation's length and the rows it had, as last drawn.
    last_total: usize,
    last_height: usize,
}

enum Watching {
    Starting,
    Live,
    Ended(Option<String>),
}

impl TalkPage {
    pub fn new(resident: ResidentNow, project: Option<String>, here: Option<MachineName>) -> Self {
        let address = fastctx_client::town::address(&resident);
        let agent = resident.about.host.agent.to_string();
        let state = resident.now.as_ref().map(|now| (now.activity, now.since));
        let mut page = Self::bare(address, agent, project, here);
        page.town = resident.town.clone();
        page.origin = Some(resident.about.origin.clone());
        page.model = resident.about.model.clone();
        page.state = state;
        page
    }

    /// A session just opened with the user's first words.
    pub fn opened(
        agent: String,
        town: Option<TownName>,
        project: Option<String>,
        first: String,
        said: fastctx_client::Said,
        here: MachineName,
    ) -> Self {
        let mut page = Self::bare(said.to.clone(), agent, project, Some(here.clone()));
        page.town = town;
        page.origin = Some(Origin::User);
        let local = page.transcript.sending(first, Some(here));
        page.transcript.landed(local, said.landed, said.to.machine);
        page
    }

    fn bare(address: Address, agent: String, project: Option<String>, here: Option<MachineName>) -> Self {
        Self {
            transcript: Transcript::new(here.clone()),
            address,
            agent,
            origin: None,
            model: None,
            project,
            town: None,
            running: Ok(Vec::new()),
            state: None,
            editor: Editor::multi(),
            top: None,
            watching: Watching::Starting,
            round: 0,
            stopping: false,
            note: None,
            here,
            pending: VecDeque::new(),
            last_total: 0,
            last_height: 0,
        }
    }

    pub fn feed(&self) -> Option<Feed> {
        match self.watching {
            Watching::Ended(_) => None,
            _ => Some(Feed::Watch {
                resident: self.address.resident.clone(),
                machine: self.address.machine.clone(),
                town: self.town.clone(),
                round: self.round,
            }),
        }
    }

    pub fn animating(&self) -> bool {
        self.transcript.moving()
            || matches!(self.state, Some((Activity::Busy, _)))
            || self.running.as_ref().is_ok_and(|jobs| !jobs.is_empty())
    }

    pub fn paste(&mut self, text: String) -> Act {
        self.editor.insert(&text);
        Act::Stay
    }

    pub fn data(&mut self, ctx: &Ctx, data: Data) -> Act {
        match data {
            Data::History(events) => {
                self.watching = Watching::Live;
                for event in &events {
                    self.follow_state(&event.happened, event.at);
                }
                self.transcript.history(events);
            }
            Data::Event(event) => {
                self.follow_state(&event.happened, event.at);
                // The turn's end says how the stop came out; the note
                // that it was asked for has done its work.
                if let Happened::TurnEnded { .. } = event.happened
                    && std::mem::take(&mut self.stopping)
                {
                    self.note = None;
                }
                self.transcript.event(event);
            }
            Data::Unwatched(why) => self.watching = Watching::Ended(why),
            Data::Running(running) => self.running = running,
            Data::Done { job, result } => match (job, result) {
                (Job::Talk { .. }, Ok(Done::Said(said))) => {
                    if let Some(local) = self.pending.pop_front() {
                        self.transcript.landed(local, said.landed, said.to.machine);
                    }
                }
                (Job::Talk { .. }, Err(why)) => {
                    if let Some(local) = self.pending.pop_front() {
                        self.transcript.failed(local, why);
                    }
                }
                (Job::Stop { .. }, Ok(Done::Stopped(false))) => {
                    self.stopping = false;
                    self.note = Some(quiet(ctx, ctx.words.no_turn_running()));
                }
                (Job::Stop { .. }, Ok(_)) => {}
                (Job::Stop { .. }, Err(why)) => {
                    self.stopping = false;
                    self.note = Some(wrong(ctx, ctx.words.not_stopped(&text::first_line(&why))));
                }
                _ => {}
            },
            _ => {}
        }
        Act::Stay
    }

    /// The session's state as its events tell it.
    fn follow_state(&mut self, happened: &Happened, at: Timestamp) {
        match happened {
            Happened::TurnStarted { .. } => self.state = Some((Activity::Busy, at)),
            Happened::TurnEnded { .. } => self.state = Some((Activity::Idle, at)),
            _ => {}
        }
    }

    pub fn key(&mut self, ctx: &Ctx, key: KeyEvent) -> Act {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Act::Back,
            KeyCode::Char('x') if control => {
                self.stopping = true;
                self.note = Some(quiet(ctx, ctx.words.stopping()));
                return Act::Do(Job::Stop {
                    resident: self.address.resident.clone(),
                    machine: self.address.machine.clone(),
                });
            }
            KeyCode::Char('r') if control => {
                if matches!(self.watching, Watching::Ended(_)) {
                    self.round += 1;
                    self.watching = Watching::Starting;
                }
                return Act::Stay;
            }
            KeyCode::PageUp => {
                self.scroll_by(-1);
                return Act::Stay;
            }
            KeyCode::PageDown => {
                self.scroll_by(1);
                return Act::Stay;
            }
            KeyCode::Home if control => {
                self.top = Some(0);
                return Act::Stay;
            }
            KeyCode::End if control => {
                self.top = None;
                return Act::Stay;
            }
            _ => {}
        }
        match self.editor.key(&key) {
            Edit::Enter if !self.editor.is_empty() => {
                let text = self.editor.text().to_string();
                self.editor.clear();
                self.top = None;
                self.note = None;
                let local = self.transcript.sending(text.clone(), self.here.clone());
                self.pending.push_back(local);
                Act::Do(Job::Talk { to: self.address.resident.clone(), machine: self.address.machine.clone(), text })
            }
            Edit::Unhandled => {
                match key.code {
                    // With nothing being typed above or below, the arrows
                    // read back through the conversation.
                    KeyCode::Up => self.scroll_lines(-3),
                    KeyCode::Down => self.scroll_lines(3),
                    _ => {}
                }
                Act::Stay
            }
            _ => Act::Stay,
        }
    }

    /// Pages of scrolling; a page here is the last body height drawn.
    fn scroll_by(&mut self, pages: i64) {
        self.scroll_lines(pages * self.last_height.max(4) as i64 - pages.signum() * 2);
    }

    fn scroll_lines(&mut self, lines: i64) {
        let bottom = self.last_total.saturating_sub(self.last_height);
        let top = self.top.unwrap_or(bottom) as i64 + lines;
        let top = top.clamp(0, bottom as i64) as usize;
        self.top = if top >= bottom { None } else { Some(top) };
    }

    /// A row for each thing it started that still runs: the command, then
    /// where and for how long.
    fn running_rows(&self, ctx: &Ctx) -> Vec<(Line<'static>, Line<'static>)> {
        let look = &ctx.look;
        match &self.running {
            Err(why) => vec![(quiet(ctx, ctx.words.running_unseen(&text::first_line(why))), Line::default())],
            Ok(jobs) => jobs
                .iter()
                .map(|job| {
                    let left = Line::from(vec![
                        Span::styled("◌  ", look.faded(Role::Alive, ctx.breath)),
                        Span::styled(text::first_line(&job.command), look.text(Role::Muted)),
                    ]);
                    let right = Line::from(Span::styled(
                        format!("{} · {}", job.machine, ctx.words.since(job.started_at, ctx.now)),
                        look.text(Role::Faint),
                    ));
                    (left, right)
                })
                .collect(),
        }
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect, pasting: Option<usize>) {
        let words = &ctx.words;
        let look = &ctx.look;
        let column = paint::column(area);
        let width = column.width as usize;
        let buffer = frame.buffer_mut();

        // The name, and what is true of the session now.
        let mut crumbs = vec![words.world().to_string()];
        crumbs.extend(self.project.clone());
        crumbs.push(who::name(ctx, &self.agent, &self.address.machine));
        let state = match self.state {
            Some((Activity::Busy, since)) => Line::from(vec![
                Span::styled("● ", look.faded(Role::Alive, ctx.breath)),
                Span::styled(format!("{} {}", words.busy(), words.since(since, ctx.now)), look.text(Role::Alive)),
            ]),
            Some((Activity::Idle, since)) => Line::from(vec![
                Span::styled("○ ", look.text(Role::Muted)),
                Span::styled(format!("{} {}", words.idle(), words.since(since, ctx.now)), look.text(Role::Muted)),
            ]),
            Some((Activity::Asleep, _)) => {
                Line::from(Span::styled(format!("◌ {}", words.asleep()), look.text(Role::Faint)))
            }
            None => Line::default(),
        };
        let at = |offset: u16| Rect { y: column.y + offset, height: 1, ..column };
        paint::header(buffer, at(1), look, &crumbs, &state);
        let mut about = Vec::new();
        match &self.origin {
            Some(Origin::Colleague { asked_by, label }) => {
                about.push(format!("{}{}", words.asked_by(&who::address_name(ctx, asked_by)), words.quoted(label)))
            }
            Some(Origin::User) => about.push(words.opened_by_you().to_string()),
            None => {}
        }
        about.extend(self.model.clone());
        about.push(self.address.resident.to_string());
        paint::line(
            buffer,
            column.x,
            column.y + 2,
            column.width,
            &quiet(ctx, about.join("  ·  ")),
            &Line::default(),
            false,
            look,
        );

        // The place to type, at the bottom: a rounded box, `>` before the
        // first line, the keys right under it.
        let bottom = column.height.saturating_sub(1);
        let footer = at(bottom.saturating_sub(1));
        let left = column.x.saturating_sub(2);
        let outer = (column.width + 4).min(buffer.area.right().saturating_sub(left));
        let typing_width = (outer as usize).saturating_sub(6);
        let (mut lines, caret) = self.editor.layout(typing_width, look.text(Role::Text));
        let empty = self.editor.text().is_empty();
        if empty {
            lines = vec![Line::from(Span::styled(words.say_to(&words.agent(&self.agent)), look.text(Role::Faint)))];
        }
        let most = (column.height / EDITOR_SHARE).max(1) as usize;
        let first_shown = (caret.0 + 1).saturating_sub(most);
        let shown = lines.len().min(most);
        let box_height = shown as u16 + 2;
        let box_top = footer.y.saturating_sub(box_height);
        let border = look.text(Role::Faint);
        let rule = "─".repeat((outer as usize).saturating_sub(2));
        buffer.set_string(left, box_top, format!("╭{rule}╮"), border);
        buffer.set_string(left, box_top + box_height - 1, format!("╰{rule}╯"), border);
        for row in 0..shown as u16 {
            let y = box_top + 1 + row;
            buffer.set_string(left, y, "│", border);
            buffer.set_string(left + outer - 1, y, "│", border);
        }
        buffer.set_string(left + 2, box_top + 1, ">", look.text(Role::Muted));
        for (row, line) in lines.iter().skip(first_shown).take(shown).enumerate() {
            paint::line(
                buffer,
                left + 4,
                box_top + 1 + row as u16,
                typing_width as u16,
                line,
                &Line::default(),
                false,
                look,
            );
        }

        // Above the box, what is going on: a note, or the turn at work.
        let status = at(box_top.saturating_sub(column.y + 1));
        let status_line = if let Some(count) = pasting {
            Some(quiet(ctx, words.pasting(count)))
        } else if let Watching::Ended(why) = &self.watching {
            Some(match why {
                Some(why) => wrong(ctx, words.watch_broke(&text::first_line(why))),
                None => quiet(ctx, words.watch_ended()),
            })
        } else if self.note.is_some() {
            self.note.clone()
        } else if let Some((Activity::Busy, since)) = self.state {
            let mut spans = vec![
                Span::styled("✻ ", look.faded(Role::Accent, ctx.breath)),
                Span::styled(format!("{}…", words.busy()), look.text(Role::Accent)),
                Span::styled(format!("  {}", words.since(since, ctx.now)), look.text(Role::Faint)),
            ];
            if !self.stopping {
                spans.push(Span::styled(format!("  ·  Ctrl+X {}", words.key_stop()), look.text(Role::Faint)));
            }
            Some(Line::from(spans))
        } else {
            None
        };
        if let Some(line) = status_line {
            paint::line(buffer, status.x, status.y, status.width, &line, &Line::default(), false, look);
        }

        // What it started that still runs, above that.
        let running = self.running_rows(ctx);
        let running_top = status.y.saturating_sub(running.len() as u16);
        for (row, (left, right)) in running.iter().enumerate() {
            paint::line(buffer, column.x, running_top + row as u16, column.width, left, right, false, look);
        }
        let above = running_top;

        // The conversation, above that.
        let body =
            Rect { x: column.x, y: column.y + 4, width: column.width, height: above.saturating_sub(column.y + 5) };
        let all = self.transcript.lines(ctx, width);
        let height = body.height as usize;
        self.last_total = all.len();
        self.last_height = height;
        let bottom_top = all.len().saturating_sub(height);
        let top = self.top.map_or(bottom_top, |top| top.min(bottom_top));
        for (row, line) in all.iter().skip(top).take(height).enumerate() {
            paint::line(buffer, body.x, body.y + row as u16, body.width, line, &Line::default(), false, look);
        }
        if all.is_empty() {
            let waiting = match &self.watching {
                Watching::Starting => words.opening_session(),
                _ => words.nothing_said_yet(),
            };
            paint::line(buffer, body.x, body.y, body.width, &quiet(ctx, waiting), &Line::default(), false, look);
        }
        if top < bottom_top {
            let more = Line::from(Span::styled(words.more_below(), look.text(Role::Faint)));
            let x = body.right().saturating_sub(text::line_width(&more) as u16);
            paint::line(buffer, x, body.bottom().saturating_sub(1), body.width, &more, &Line::default(), false, look);
        }

        // The keys.
        let mut keys =
            vec![("Enter", words.key_send()), ("Alt+Enter", words.key_newline()), ("PgUp PgDn", words.key_scroll())];
        if matches!(self.watching, Watching::Ended(_)) {
            keys.push(("Ctrl+R", words.key_watch_again()));
        }
        keys.push(("Esc", words.key_back()));
        paint::footer(buffer, footer, look, &keys);

        if !empty {
            let row = caret.0.saturating_sub(first_shown) as u16;
            frame.set_cursor_position(Position::new(left + 4 + caret.1 as u16, box_top + 1 + row));
        }
    }
}
