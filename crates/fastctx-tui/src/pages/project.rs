//! One project (`world-gui.md`: a town is a project; its home is the
//! overview): what it is for, the agents in it and what each is doing, its
//! journals, its decisions, and where things are.

use crossterm::event::{KeyCode, KeyEvent};
use fastctx_client::TownView;
use fastctx_types::TownName;
use fastctx_types::project::{MapView, Writer};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{List, Page, Sheet, quiet, sheet, title, wrong};
use crate::app::{Act, Ctx, Data, Feed};
use crate::design::Role;
use crate::pages::{form, journal, talk, world};
use crate::paint::{self, Row};
use crate::{text, who};

pub struct ProjectPage {
    town: TownName,
    view: Option<TownView>,
    map: Option<MapView>,
    failed: Option<String>,
    had_at: Option<Timestamp>,
    all: bool,
    list: List,
}

enum Item {
    Resident(usize),
    Journal(usize),
}

impl ProjectPage {
    pub fn new(town: TownName) -> Self {
        Self { town, view: None, map: None, failed: None, had_at: None, all: false, list: List::default() }
    }

    /// The page as it would look with these answers, for previews.
    pub fn with(town: TownName, view: TownView, map: MapView) -> Self {
        Self { view: Some(view), map: Some(map), had_at: Some(Timestamp::now()), ..Self::new(town) }
    }

    pub fn feed(&self) -> Feed {
        Feed::Town { town: self.town.clone(), all: self.all }
    }

    pub fn animating(&self) -> bool {
        self.view.as_ref().is_some_and(|view| view.residents.iter().any(fastctx_client::town::busy))
    }

    fn items(&self) -> Vec<Item> {
        let residents = self.view.as_ref().map_or(0, |view| view.residents.len());
        let journals = self.map.as_ref().map_or(0, |map| map.journals.len());
        (0..residents).map(Item::Resident).chain((0..journals).map(Item::Journal)).collect()
    }

    /// The project's name, as its record gives it.
    fn name(&self) -> String {
        self.map.as_ref().map(|map| map.record.name.clone()).unwrap_or_else(|| self.town.to_string())
    }

    pub fn data(&mut self, data: Data) -> Act {
        if let Data::Town { view, map } = data {
            let mut failures = Vec::new();
            match view {
                Ok(view) => self.view = Some(view),
                Err(error) => failures.push(error),
            }
            match map {
                Ok(map) => self.map = Some(*map),
                Err(error) => failures.push(error),
            }
            if failures.is_empty() {
                self.had_at = Some(Timestamp::now());
                self.failed = None;
            } else {
                self.failed = Some(failures.join("; "));
            }
            let count = self.items().len();
            self.list.clamp(count);
        }
        Act::Stay
    }

    pub fn key(&mut self, _ctx: &Ctx, key: KeyEvent) -> Act {
        let items = self.items();
        if self.list.key(&key, items.len()) {
            return Act::Stay;
        }
        match key.code {
            KeyCode::Enter => match items.get(self.list.selected) {
                Some(Item::Resident(index)) => {
                    let view = self.view.as_ref().expect("items come from the view");
                    let page = talk::TalkPage::new(
                        view.residents[*index].clone(),
                        Some(self.name()),
                        Some(view.machine.clone()),
                    );
                    Act::Push(Page::Talk(Box::new(page)))
                }
                Some(Item::Journal(index)) => {
                    let map = self.map.as_ref().expect("items come from the map");
                    let journal = map.journals[*index].journal.clone();
                    let page = journal::JournalPage::new(self.town.clone(), self.name(), journal);
                    Act::Push(Page::Journal(Box::new(page)))
                }
                None => Act::Stay,
            },
            KeyCode::Char('n') => Act::Push(Page::Form(Box::new(form::FormPage::open_agent(Some(self.town.clone()))))),
            KeyCode::Char('e') => match &self.map {
                Some(map) => {
                    Act::Push(Page::Form(Box::new(form::FormPage::edit_project(self.town.clone(), &map.record))))
                }
                None => Act::Stay,
            },
            KeyCode::Char('a') => {
                self.all = !self.all;
                Act::Stay
            }
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => Act::Back,
            _ => Act::Stay,
        }
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect) {
        let words = &ctx.words;
        let look = &ctx.look;
        let width = paint::column(area).width as usize;
        let mut rows = Vec::new();
        let mut item = 0;

        if let Some(map) = &self.map {
            let record = &map.record;
            for line in text::wrap_plain(&record.what, look.text(Role::Text), width) {
                rows.push(Row::new(line));
            }
            let machines: Vec<&str> = record.machines.iter().map(|machine| machine.as_str()).collect();
            let opened = words.opened(
                &writer_name(ctx, &map.opened.by),
                &crate::words::clock(map.opened.at, ctx.now),
                record.version,
            );
            rows.push(Row::new(quiet(ctx, format!("{}  ·  {}  ·  {opened}", self.town, machines.join(" · ")))));
            rows.push(Row::blank());
        }

        if let Some(view) = &self.view {
            let (here, busy) = view.counts();
            let count = if busy > 0 {
                Span::styled(words.here_busy(here, busy), look.text(Role::Alive))
            } else {
                Span::styled(words.here_busy(here, 0), look.text(Role::Muted))
            };
            paint::open_section(&mut rows, paint::section(words.agents(), look).right(count));
            if view.residents.is_empty() {
                rows.push(Row::new(quiet(ctx, words.no_agents_here())));
                rows.push(Row::blank());
            }
            for resident in &view.residents {
                let (left, right) = who::first_line(ctx, resident);
                rows.push(Row::new(left).right(right).item(item));
                let jobs = view.jobs_of(resident);
                rows.push(Row::new(who::second_line(ctx, resident, jobs.last().copied())).item(item));
                rows.push(Row::blank());
                item += 1;
            }
            if let fastctx_client::town::Scope::Town { record: None, .. } = &view.scope {
                rows.push(Row::new(quiet(ctx, words.no_record_here(view.machine.as_str()))));
            }
        }

        if let Some(map) = &self.map {
            paint::open_section(
                &mut rows,
                paint::section(words.journals(), look).right(quiet(ctx, words.journal_count(map.journals.len()))),
            );
            if map.journals.is_empty() {
                rows.push(Row::new(quiet(ctx, words.nothing_yet())));
            }
            for journal in &map.journals {
                let right = quiet(ctx, words.entries(journal.entries, journal.writers));
                rows.push(Row::new(title(ctx, journal.journal.to_string())).right(right).item(item));
                if let Some(description) = &journal.description {
                    let lines = text::wrap_plain(&description.text, look.text(Role::Muted), width - 4);
                    rows.extend(paint::indented(lines.into_iter().take(2).collect(), 2, Some(item)));
                }
                if let Some(latest) = &journal.latest {
                    let by = writer_name(ctx, &latest.by);
                    let line = quiet(ctx, words.last_entry(&by, &crate::words::clock(latest.at, ctx.now)));
                    rows.extend(paint::indented(vec![line], 2, Some(item)));
                }
                rows.push(Row::blank());
                item += 1;
            }

            paint::open_section(&mut rows, paint::section(words.decisions(), look));
            match (&map.record.canon, &map.canon) {
                (None, _) => rows.push(Row::new(quiet(ctx, words.no_decisions()))),
                (Some(location), canon) => {
                    rows.push(Row::new(quiet(ctx, format!("{}  {}", location.machine, location.path))));
                    match canon {
                        Some(Ok(heads)) => {
                            for head in heads {
                                let line = Line::from(vec![
                                    Span::styled(head.title.clone(), look.text(Role::Text)),
                                    Span::styled(format!("   {}", head.scope), look.text(Role::Muted)),
                                ]);
                                rows.push(Row::new(line));
                            }
                        }
                        Some(Err(why)) => rows.push(Row::new(Span::styled(
                            words.decisions_unread(location.machine.as_str(), &text::first_line(why)),
                            look.text(Role::Caution),
                        ))),
                        None => {}
                    }
                }
            }
            rows.push(Row::blank());

            if !map.record.places.is_empty() {
                paint::open_section(&mut rows, paint::section(words.places(), look));
                let what_width = map.record.places.iter().map(|place| text::width(&place.what)).max().unwrap_or(0);
                let machine_width =
                    map.record.places.iter().map(|place| place.location.machine.as_str().len()).max().unwrap_or(0);
                for place in &map.record.places {
                    let pad = what_width - text::width(&place.what);
                    rows.push(Row::new(Line::from(vec![
                        Span::styled(format!("{}{}   ", place.what, " ".repeat(pad)), look.text(Role::Text)),
                        Span::styled(
                            format!("{:machine_width$}   ", place.location.machine.as_str()),
                            look.text(Role::Muted),
                        ),
                        Span::styled(place.location.path.clone(), look.text(Role::Link)),
                    ])));
                }
                rows.push(Row::blank());
            }
        }

        if let Some(view) = &self.view {
            let unattended = view.unattended();
            if !unattended.is_empty() {
                paint::open_section(&mut rows, paint::section(words.unattended(), look));
                for job in unattended {
                    rows.push(
                        Row::new(Line::from(vec![
                            Span::styled(format!("{}   ", job.job), look.text(Role::Faint)),
                            Span::styled(text::first_line(&job.command), look.text(Role::Text)),
                        ]))
                        .right(quiet(
                            ctx,
                            format!(
                                "{} · {} · {}",
                                job.machine,
                                words.since(job.started_at, ctx.now),
                                words.started_by(&who::address_name(ctx, &job.owner))
                            ),
                        )),
                    );
                }
                rows.push(Row::blank());
            }
            paint::open_section(&mut rows, paint::section(words.machines(), look));
            rows.push(Row::new(world::machines_line(ctx, &view.machines)));
        }

        let note = match (&self.failed, self.had_at) {
            (Some(error), Some(at)) => Some(wrong(
                ctx,
                format!("{}  ·  {}", text::first_line(error), words.as_of(&crate::words::clock(at, ctx.now))),
            )),
            (Some(error), None) => Some(wrong(ctx, text::first_line(error))),
            (None, None) => Some(quiet(ctx, words.asking())),
            (None, Some(_)) => None,
        };
        let selected_journal = matches!(self.items().get(self.list.selected), Some(Item::Journal(_)));
        let page = Sheet {
            crumbs: vec![words.world().to_string(), self.name()],
            right: Line::default(),
            rows,
            keys: vec![
                ("Enter", if selected_journal { words.key_read() } else { words.key_talk() }),
                ("n", words.key_new_agent_here()),
                ("e", words.key_edit_project()),
                ("a", if self.all { words.key_present_only() } else { words.key_all() }),
                ("Esc", words.key_back()),
            ],
            note,
        };
        sheet(ctx, frame, area, &page, &mut self.list);
    }
}

/// Who wrote something into the ledger, as the user reads it.
pub fn writer_name(ctx: &Ctx, writer: &Writer) -> String {
    match writer {
        Writer::User => ctx.words.you().to_string(),
        Writer::Resident { address } => who::address_name(ctx, address),
        Writer::Session { agent } => ctx.words.agent(agent.as_str()),
    }
}
