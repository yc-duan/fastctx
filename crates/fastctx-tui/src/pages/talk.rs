//! Talking to one agent on any machine (`world-colleagues.md`, 用户在任何
//! 地方和居民说话): what came before in its session, then everything as it
//! happens, and a place to type below it. Words typed here go in as the
//! user's; the agent can be told to stop its turn.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_types::residents::{Activity, Landed, Origin};
use fastctx_types::session::{Happened, SessionChange};
use fastctx_types::town::ResidentNow;
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};

use super::{Page, quiet, wrong};
use crate::app::{Act, Around, Ctx, Data, Done, Feed, Job};
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
    /// What it started that still runs and the colleagues it asked for,
    /// wherever they are.
    around: Result<Around, String>,
    /// The colleague picked among them, while the keys go to that list
    /// rather than to the place to type. Known by who it is: the list is
    /// asked for again and again, and its order changes as they work.
    picked: Option<Address>,
    /// What its host last reported it doing, and since when.
    state: Option<(Activity, Timestamp)>,
    waiting_for_user: bool,
    /// The World started it, and brings its host back itself: words to it
    /// never wait for its next call of a World tool (`world-lifecycle.md`).
    held_by_world: bool,
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
    /// A turn that ended while watched, to tell the user of when the
    /// terminal is not in front: a title and a line.
    notice: Option<(String, String)>,
}

enum Watching {
    Starting,
    Live,
    /// Broken and being opened again: why, and since when.
    Interrupted(String, std::time::SystemTime),
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
        page.waiting_for_user = resident.now.as_ref().is_some_and(|now| now.waiting_for_user);
        page.held_by_world = resident.about.world_started;
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
        page.held_by_world = true;
        let local = page.transcript.sending(first, Some(here));
        if page.stands(&said.landed) {
            page.transcript.landed(local, said.landed, said.to.machine);
        }
        page
    }

    /// Whether what the World said of words sent stands under them. Words a
    /// conversation the World started waits on its host for, with no reason
    /// given, stay on their way while the World brings the host back.
    fn stands(&self, landed: &Landed) -> bool {
        !(self.held_by_world && matches!(landed, Landed::Waiting { why: None, .. }))
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
            around: Ok(Around::default()),
            picked: None,
            state: None,
            waiting_for_user: false,
            held_by_world: false,
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
            notice: None,
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
            || (matches!(self.state, Some((Activity::Busy, _))) && !self.waiting_for_user)
            || self
                .around
                .as_ref()
                .is_ok_and(|around| !around.jobs.is_empty() || around.colleagues.iter().any(fastctx_client::town::busy))
    }

    pub fn paste(&mut self, text: String) -> Act {
        self.picked = None;
        self.editor.insert(&text);
        Act::Stay
    }

    /// What to tell the user of a turn that ended, once.
    pub fn take_notice(&mut self) -> Option<(String, String)> {
        self.notice.take()
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
                if let Happened::TurnEnded { end, .. } = &event.happened {
                    self.notice = Some((who::name(ctx, &self.agent, &self.address.machine), ended_line(ctx, end)));
                }
                self.transcript.event(event);
            }
            Data::Unwatched(why) => self.watching = Watching::Ended(why),
            Data::Interrupted { why, since } => self.watching = Watching::Interrupted(why, since),
            Data::Current => {
                if matches!(self.watching, Watching::Interrupted(..)) {
                    self.watching = Watching::Live;
                }
            }
            Data::Returned(Ok(Some(0))) => {}
            Data::Returned(Ok(code)) => self.note = Some(quiet(ctx, ctx.words.attach_exited(code))),
            Data::Returned(Err(why)) => {
                self.note = Some(wrong(ctx, ctx.words.attach_failed(&text::first_line(&why))));
            }
            Data::Around(around) => {
                self.around = around;
                // One who is gone from the list leaves the pick at the first.
                if self.picked.is_some() && self.picked_index().is_none() {
                    self.picked = self.colleagues().first().map(fastctx_client::town::address);
                }
            }
            Data::Done { job, result } => match (job, result) {
                (Job::Talk { .. }, Ok(Done::Said(said))) => {
                    if let Some(local) = self.pending.pop_front()
                        && self.stands(&said.landed)
                    {
                        self.transcript.landed(local, said.landed, said.to.machine);
                    }
                }
                (Job::Talk { .. }, Err(_)) => {
                    if let Some(local) = self.pending.pop_front() {
                        self.transcript.failed(local);
                    }
                }
                (Job::Stop { .. }, Ok(Done::Stopped(false))) => {
                    self.stopping = false;
                    self.note = Some(quiet(ctx, ctx.words.no_turn_running()));
                }
                (Job::Stop { .. }, Ok(_)) => {}
                (Job::Stop { .. }, Err(_)) => {
                    self.stopping = false;
                    self.note = Some(wrong(ctx, ctx.words.not_stopped().to_string()));
                }
                (Job::Attach { .. }, Ok(Done::Attachable(command))) => return Act::HandOver(command),
                (Job::Attach { .. }, Err(why)) => {
                    self.note = Some(wrong(ctx, ctx.words.attach_failed(&text::first_line(&why))));
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
            Happened::TurnStarted { .. } => {
                self.state = Some((Activity::Busy, at));
                self.waiting_for_user = false;
                // A new turn makes an old note stale, and the turn at work
                // shows where the note was; a stop asked for still shows.
                if !self.stopping {
                    self.note = None;
                }
            }
            Happened::TurnEnded { .. } => {
                self.state = Some((Activity::Idle, at));
                self.waiting_for_user = false;
            }
            Happened::Changed { change: SessionChange::Status { waiting_for_user: Some(waiting), .. }, .. } => {
                self.waiting_for_user = *waiting;
            }
            _ => {}
        }
    }

    fn colleagues(&self) -> &[ResidentNow] {
        self.around.as_ref().map(|around| around.colleagues.as_slice()).unwrap_or_default()
    }

    /// Where the picked colleague sits in the list as it is now.
    fn picked_index(&self) -> Option<usize> {
        let picked = self.picked.as_ref()?;
        self.colleagues().iter().position(|colleague| fastctx_client::town::address(colleague) == *picked)
    }

    /// Keys while a colleague is picked: the arrows move among them, Enter
    /// goes to the one picked, and Tab or Esc returns to the place to type.
    /// Anything else is typing, and goes there too.
    fn pick_key(&mut self, key: &KeyEvent) -> Option<Act> {
        let count = self.colleagues().len();
        let index = self.picked_index().unwrap_or(0);
        let pick = |index: usize| self.colleagues().get(index).map(fastctx_client::town::address);
        match key.code {
            KeyCode::Up => self.picked = pick(index.saturating_sub(1)),
            KeyCode::Down => self.picked = pick((index + 1).min(count.saturating_sub(1))),
            KeyCode::Enter => {
                let colleague = self.colleagues().get(index)?.clone();
                self.picked = None;
                let page = TalkPage::new(colleague, self.project.clone(), self.here.clone());
                return Some(Act::Push(Page::Talk(Box::new(page))));
            }
            KeyCode::Tab | KeyCode::Esc => self.picked = None,
            _ => {
                self.picked = None;
                return None;
            }
        }
        Some(Act::Stay)
    }

    pub fn key(&mut self, ctx: &Ctx, key: KeyEvent) -> Act {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.picked.is_some()
            && let Some(act) = self.pick_key(&key)
        {
            return act;
        }
        match key.code {
            KeyCode::Esc => return Act::Back,
            // Tab goes to the colleagues only from an empty place to type,
            // so that while typing it stays what it always was.
            KeyCode::Tab if self.editor.is_empty() && !self.colleagues().is_empty() => {
                self.picked = self.colleagues().first().map(fastctx_client::town::address);
                return Act::Stay;
            }
            KeyCode::Char('x') if control => {
                self.stopping = true;
                self.note = Some(quiet(ctx, ctx.words.stopping()));
                return Act::Do(Job::Stop {
                    resident: self.address.resident.clone(),
                    machine: self.address.machine.clone(),
                });
            }
            KeyCode::Char('o') if control => {
                self.transcript.toggle_details();
                return Act::Stay;
            }
            KeyCode::Char('t') if control => {
                // Its host's own interface is reached on the machine it lives on.
                if self.here.as_ref() != Some(&self.address.machine) {
                    self.note = Some(quiet(ctx, ctx.words.attach_elsewhere(self.address.machine.as_str())));
                    return Act::Stay;
                }
                return Act::Do(Job::Attach { resident: self.address.resident.clone() });
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

    /// A row for each colleague it asked for, then for each thing it
    /// started that still runs: who or what, then how it stands.
    fn around_rows(&self, ctx: &Ctx) -> Vec<paint::Row> {
        let look = &ctx.look;
        let words = &ctx.words;
        let around = match &self.around {
            Ok(around) => around,
            Err(why) => return vec![paint::Row::new(quiet(ctx, words.running_unseen(&text::first_line(why))))],
        };
        let mut rows = Vec::new();
        for (item, colleague) in around.colleagues.iter().enumerate() {
            let mut left = vec![
                who::mark(ctx, colleague),
                Span::raw("  "),
                Span::styled(who::resident_name(ctx, colleague), look.text(Role::Text)),
            ];
            if let Origin::Colleague { label, .. } = &colleague.about.origin {
                left.push(Span::styled(format!("  {}", words.quoted(label)), look.text(Role::Muted)));
            }
            let state = match colleague.now.as_ref().map(|now| (now.activity, now.since)) {
                _ if !colleague.here => words.left().to_string(),
                Some((Activity::Busy, _)) if colleague.now.as_ref().is_some_and(|now| now.waiting_for_user) => {
                    words.waiting_for_user().to_string()
                }
                Some((Activity::Busy, since)) => format!("{} {}", words.busy(), words.since(since, ctx.now)),
                Some((Activity::Idle, since)) => format!("{} {}", words.idle(), words.since(since, ctx.now)),
                Some((Activity::Asleep, _)) => words.asleep().to_string(),
                None => String::new(),
            };
            rows.push(
                paint::Row::new(Line::from(left))
                    .right(Line::from(Span::styled(state, look.text(Role::Faint))))
                    .item(item),
            );
        }
        for job in &around.jobs {
            let left = Line::from(vec![
                Span::styled("◌  ", look.faded(Role::Alive, ctx.breath)),
                Span::styled(text::first_line(&job.command), look.text(Role::Muted)),
            ]);
            let right = Line::from(Span::styled(
                format!("{} · {}", job.machine, words.since(job.started_at, ctx.now)),
                look.text(Role::Faint),
            ));
            rows.push(paint::Row::new(left).right(right));
        }
        rows
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
        // A turn at work shows above the place to type, where the eye is.
        let mut state = match self.state {
            Some((Activity::Busy, _)) => Line::default(),
            Some((Activity::Idle, since)) => Line::from(vec![
                Span::styled("○ ", look.text(Role::Muted)),
                Span::styled(format!("{} {}", words.idle(), words.since(since, ctx.now)), look.text(Role::Muted)),
            ]),
            Some((Activity::Asleep, _)) => {
                Line::from(Span::styled(format!("◌ {}", words.asleep()), look.text(Role::Faint)))
            }
            None => Line::default(),
        };
        // How full its context is, beside how it stands.
        if let Some((used, window)) = self.transcript.context() {
            if !state.spans.is_empty() {
                state.spans.push(Span::styled("  ·  ", look.text(Role::Faint)));
            }
            state.spans.push(Span::styled(words.context_used(used, window), look.text(Role::Muted)));
        }
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
        if let Some(title) = self.transcript.title() {
            about.push(words.quoted(title));
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

        // The place to type, at the bottom: a rounded box, `❯` before the
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
        let layout = text::Layout::default();
        layout.write_line(buffer, Rect::new(left, box_top, outer, 1), &Line::styled(format!("╭{rule}╮"), border));
        layout.write_line(
            buffer,
            Rect::new(left, box_top + box_height - 1, outer, 1),
            &Line::styled(format!("╰{rule}╯"), border),
        );
        for row in 0..shown as u16 {
            let y = box_top + 1 + row;
            layout.write_line(buffer, Rect::new(left, y, 1, 1), &Line::styled("│", border));
            layout.write_line(buffer, Rect::new(left + outer - 1, y, 1, 1), &Line::styled("│", border));
        }
        layout.write_line(buffer, Rect::new(left + 2, box_top + 1, 1, 1), &Line::styled("❯", look.text(Role::Muted)));
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
        } else if let Watching::Interrupted(why, since) = &self.watching {
            let lasted = since.elapsed().map(|lasted| lasted.as_secs()).unwrap_or_default();
            Some(wrong(ctx, words.watch_interrupted(&words.duration(lasted), &text::first_line(why))))
        } else if self.note.is_some() {
            self.note.clone()
        } else if self.waiting_for_user {
            Some(quiet(ctx, words.waiting_for_user()))
        } else if let Some((Activity::Busy, since)) = self.state {
            // Braille turns in one cell, in the terminal's own font.
            const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut spans = vec![
                Span::styled(SPINNER[(ctx.frame % SPINNER.len() as u64) as usize], look.text(Role::Accent)),
                Span::styled(format!("  {}", words.since(since, ctx.now)), look.text(Role::Muted)),
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

        // Its colleagues and what it started that still runs, above that,
        // in at most half the room left for the conversation.
        let mut around = self.around_rows(ctx);
        let room = (status.y.saturating_sub(column.y + 5) / 2).max(1) as usize;
        let picked = self.picked_index();
        if around.len() > room {
            // Rows keep the colleague they belong to, so the pick still
            // finds its row after the list is cut.
            let shown = room - 1;
            let first = picked.map_or(0, |picked| (picked + 1).saturating_sub(shown));
            let hidden = around.len() - shown;
            around = around.into_iter().skip(first).take(shown).collect();
            around.push(paint::Row::new(quiet(ctx, words.more_rows(hidden))));
        }
        let around_top = status.y.saturating_sub(around.len() as u16);
        let area = Rect { y: around_top, height: around.len() as u16, ..column };
        paint::rows(buffer, area, &around, 0, picked, look);
        let above = around_top;

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
        let keys = if self.picked.is_some() {
            vec![("Enter", words.key_open_colleague()), ("↑↓", words.key_choose()), ("Esc", words.key_back_to_typing())]
        } else {
            let mut keys =
                vec![("Enter", words.key_send()), ("Ctrl+J", words.key_newline()), ("PgUp PgDn", words.key_scroll())];
            if empty && !self.colleagues().is_empty() {
                keys.push(("Tab", words.key_colleagues()));
            }
            if self.transcript.has_tools() {
                keys.push(("Ctrl+O", words.key_details(self.transcript.details())));
            }
            if matches!(self.watching, Watching::Ended(_)) {
                keys.push(("Ctrl+R", words.key_watch_again()));
            }
            if self.here.as_ref() == Some(&self.address.machine) {
                keys.push(("Ctrl+T", words.key_attach()));
            }
            keys.push(("Esc", words.key_back()));
            keys
        };
        paint::footer(buffer, footer, look, &keys);

        if !empty && self.picked.is_none() {
            let row = caret.0.saturating_sub(first_shown) as u16;
            frame.set_cursor_position(Position::new(left + 4 + caret.1 as u16, box_top + 1 + row));
        }
    }
}

/// A turn's end in a line: its last message's first line, or how it ended
/// when it did not complete.
fn ended_line(ctx: &Ctx, end: &fastctx_types::session::TurnEnd) -> String {
    use fastctx_types::session::TurnHow;
    let words = &ctx.words;
    let reason = end.reason.as_deref().map(text::first_line);
    match &end.how {
        TurnHow::Completed => {
            end.last_message.as_deref().map(text::first_line).unwrap_or_else(|| words.turn_done().to_string())
        }
        TurnHow::Interrupted if reason.is_none() => words.turn_stopped().to_string(),
        TurnHow::Interrupted => words.turn_interrupted(reason.as_deref()),
        TurnHow::Failed => words.turn_failed(reason.as_deref()),
        TurnHow::Other(how) => words.turn_ended_as(how),
    }
}
