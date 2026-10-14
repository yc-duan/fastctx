//! The World: every project with who is in it, the agents in none, what
//! still runs with nobody here who started it, and the machines. Projects
//! and agents belong to conversations, and show only while they are on.

use crossterm::event::{KeyCode, KeyEvent};
use fastctx_client::TownView;
use fastctx_client::town::{MachineState, Scope};
use fastctx_types::world::{LinkState, NodesReply};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{List, Page, Sheet, quiet, sheet, title, wrong};
use crate::app::{Act, Ctx, Data, Feed};
use crate::design::Role;
use crate::pages::{form, machines, project, talk};
use crate::paint::{self, Row};
use crate::sessions::Standing;
use crate::{text, who};

#[derive(Default)]
pub struct WorldPage {
    view: Option<TownView>,
    nodes: Option<NodesReply>,
    /// Why the latest ask failed, and when the shown view was had.
    failed: Option<String>,
    had_at: Option<Timestamp>,
    all: bool,
    list: List,
    /// Whether an old fastctx's tools are still in this machine's Codex.
    old_tools: bool,
    program: Option<Result<fastctx_types::renewal::Status, String>>,
    /// Whether conversations were on as the page was last drawn.
    conversations: bool,
}

/// What an item of the page is.
enum Item {
    Town(usize),
    Resident(usize),
}

impl WorldPage {
    pub fn new() -> Self {
        Self::default()
    }

    /// The page as it would look with these answers, for previews.
    pub fn with(view: TownView, nodes: NodesReply) -> Self {
        Self { view: Some(view), nodes: Some(nodes), had_at: Some(Timestamp::now()), ..Self::default() }
    }

    pub fn feed(&self) -> Feed {
        Feed::World { all: self.all }
    }

    pub fn animating(&self) -> bool {
        self.view.as_ref().is_some_and(|view| view.residents.iter().any(fastctx_client::town::busy))
    }

    fn items(&self) -> Vec<Item> {
        let Some(view) = self.view.as_ref().filter(|_| self.conversations) else { return Vec::new() };
        let towns = match &view.scope {
            Scope::World { towns } => towns.len(),
            Scope::Town { .. } => 0,
        };
        (0..towns).map(Item::Town).chain((0..view.residents.len()).map(Item::Resident)).collect()
    }

    pub fn data(&mut self, data: Data) -> Act {
        if let Data::Done { job: crate::app::Job::Conversations { .. }, result: Err(why) } = data {
            self.failed = Some(why);
            return Act::Stay;
        }
        if let Data::Program(status) = data {
            self.program = Some(status);
            return Act::Stay;
        }
        if let Data::Sessions(Ok(status)) = &data {
            self.old_tools = status.codex == Standing::Old;
        }
        if let Data::World { view, nodes } = data {
            match view {
                Ok(mut view) => {
                    if let Scope::World { towns } = &mut view.scope {
                        towns.sort_by(|a, b| (b.busy, b.here, &a.town).cmp(&(a.busy, a.here, &b.town)));
                    }
                    self.view = Some(view);
                    self.failed = None;
                    self.had_at = Some(Timestamp::now());
                }
                Err(error) => self.failed = Some(error),
            }
            if let Ok(nodes) = nodes {
                self.nodes = Some(nodes);
            }
            let count = self.items().len();
            self.list.clamp(count);
        }
        Act::Stay
    }

    pub fn key(&mut self, ctx: &Ctx, key: KeyEvent) -> Act {
        self.conversations = ctx.conversations;
        let items = self.items();
        if self.list.key(&key, items.len()) {
            return Act::Stay;
        }
        match key.code {
            KeyCode::Char('c') => Act::Do(crate::app::Job::Conversations { on: !self.conversations }),
            KeyCode::Enter | KeyCode::Char('n' | 'p' | 'a') if !self.conversations => Act::Stay,
            KeyCode::Enter => match (items.get(self.list.selected), &self.view) {
                (Some(Item::Town(index)), Some(TownView { scope: Scope::World { towns }, .. })) => {
                    Act::Push(Page::Project(Box::new(project::ProjectPage::new(towns[*index].town.clone()))))
                }
                (Some(Item::Resident(index)), Some(view)) => Act::Push(Page::Talk(Box::new(talk::TalkPage::new(
                    view.residents[*index].clone(),
                    None,
                    Some(view.machine.clone()),
                )))),
                _ => Act::Stay,
            },
            KeyCode::Char('n') => Act::Push(Page::Form(Box::new(form::FormPage::open_agent(None)))),
            KeyCode::Char('p') => Act::Push(Page::Form(Box::new(form::FormPage::new_project()))),
            KeyCode::Char('m') => Act::Push(Page::Machines(Box::new(machines::MachinesPage::new()))),
            KeyCode::Char('a') => {
                self.all = !self.all;
                Act::Stay
            }
            KeyCode::Char('q') | KeyCode::Esc => Act::Quit,
            _ => Act::Stay,
        }
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect) {
        self.conversations = ctx.conversations;
        let words = &ctx.words;
        let width = paint::column(area).width as usize;
        let mut rows = Vec::new();
        if let Some(program) = &self.program {
            rows.extend(super::program::rows(ctx, program, width));
        }
        let mut item = 0;
        if let Some(view) = &self.view {
            if let Scope::World { towns } = &view.scope
                && self.conversations
            {
                paint::open_section(&mut rows, paint::section(words.projects(), &ctx.look));
                if towns.is_empty() {
                    rows.push(Row::new(quiet(ctx, words.no_projects())));
                }
                for town in towns {
                    let name = town.summary.as_ref().map(|summary| summary.record.name.clone());
                    let mut left = vec![title(ctx, name.clone().unwrap_or_else(|| town.town.to_string()))];
                    if name.as_deref() != Some(town.town.as_str()) {
                        left.push(Span::styled(format!("   {}", town.town), ctx.look.text(Role::Faint)));
                    }
                    let count = if town.here == 0 {
                        Span::styled(words.nobody_here(), ctx.look.text(Role::Faint))
                    } else if town.busy > 0 {
                        Span::styled(words.here_busy(town.here, town.busy), ctx.look.text(Role::Alive))
                    } else {
                        Span::styled(words.here_busy(town.here, 0), ctx.look.text(Role::Muted))
                    };
                    rows.push(Row::new(Line::from(left)).right(count).item(item));
                    match &town.summary {
                        Some(summary) => {
                            let what = text::wrap_plain(&summary.record.what, ctx.look.text(Role::Muted), width - 4);
                            rows.extend(paint::indented(what.into_iter().take(2).collect(), 2, Some(item)));
                            let machines: Vec<&str> =
                                summary.record.machines.iter().map(|machine| machine.as_str()).collect();
                            rows.extend(paint::indented(vec![quiet(ctx, machines.join(" · "))], 2, Some(item)));
                        }
                        None => rows.extend(paint::indented(
                            vec![quiet(ctx, words.no_ledger_here(view.machine.as_str()))],
                            2,
                            Some(item),
                        )),
                    }
                    rows.push(Row::blank());
                    item += 1;
                }
            }
            if !view.residents.is_empty() && self.conversations {
                paint::open_section(&mut rows, paint::section(words.outside(), &ctx.look));
                for resident in &view.residents {
                    let (left, right) = who::first_line(ctx, resident);
                    rows.push(Row::new(left).right(right).item(item));
                    let jobs = view.jobs_of(resident);
                    rows.push(Row::new(who::second_line(ctx, resident, jobs.last().copied())).item(item));
                    rows.push(Row::blank());
                    item += 1;
                }
            }
            let unattended = view.unattended();
            if !unattended.is_empty() {
                paint::open_section(&mut rows, paint::section(words.unattended(), &ctx.look));
                for job in unattended {
                    let line = Line::from(vec![
                        Span::styled(format!("{}   ", job.job), ctx.look.text(Role::Faint)),
                        Span::styled(text::first_line(&job.command), ctx.look.text(Role::Text)),
                    ]);
                    let right = Line::from(Span::styled(
                        format!(
                            "{} · {} · {}",
                            job.machine,
                            words.since(job.started_at, ctx.now),
                            words.started_by(
                                &job.owner
                                    .resident()
                                    .map(|owner| who::address_name(ctx, owner))
                                    .unwrap_or_else(|| job.owner.to_string())
                            )
                        ),
                        ctx.look.text(Role::Faint),
                    ));
                    rows.push(Row::new(line).right(right));
                }
                rows.push(Row::blank());
            }
            paint::open_section(&mut rows, paint::section(words.machines(), &ctx.look));
            rows.push(Row::new(machines_line(ctx, &view.machines)));
        }
        let note = match (&self.failed, &self.view, self.had_at) {
            (Some(error), Some(_), Some(at)) => Some(wrong(
                ctx,
                format!(
                    "{}{}{}  ·  {}",
                    words.node_unreachable(),
                    words.colon(),
                    text::first_line(error),
                    words.as_of(&crate::words::clock(at, ctx.now))
                ),
            )),
            (Some(error), _, _) => {
                Some(wrong(ctx, format!("{}{}{}", words.node_unreachable(), words.colon(), text::first_line(error))))
            }
            (None, None, _) => Some(quiet(ctx, words.asking())),
            _ if self.old_tools => Some(Line::from(Span::styled(words.old_tools(), ctx.look.text(Role::Caution)))),
            _ => None,
        };
        let page = Sheet {
            crumbs: vec![words.world().to_string()],
            right: link_line(ctx, self.nodes.as_ref()),
            rows,
            keys: if self.conversations {
                vec![
                    ("Enter", words.key_open()),
                    ("n", words.key_new_agent()),
                    ("p", words.key_new_project()),
                    ("m", words.key_machines()),
                    ("a", if self.all { words.key_present_only() } else { words.key_all() }),
                    ("c", words.key_conversations(true)),
                    ("q", words.key_quit()),
                ]
            } else {
                vec![("m", words.key_machines()), ("c", words.key_conversations(false)), ("q", words.key_quit())]
            },
            note,
        };
        sheet(ctx, frame, area, &page, &mut self.list);
    }
}

/// Each machine asked, and whether it answered.
pub fn machines_line(ctx: &Ctx, machines: &[MachineState]) -> Line<'static> {
    let words = &ctx.words;
    let mut spans = Vec::new();
    for (index, state) in machines.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("     "));
        }
        match state {
            MachineState::Answered(machine) => {
                spans.push(Span::styled("● ", ctx.look.text(Role::Alive)));
                spans.push(Span::styled(machine.to_string(), ctx.look.text(Role::Text)));
            }
            MachineState::Offline { machine, since } => {
                spans.push(Span::styled("○ ", ctx.look.text(Role::Faint)));
                spans.push(Span::styled(machine.to_string(), ctx.look.text(Role::Muted)));
                let offline = match since {
                    Some(since) => words.offline_since(&crate::words::clock(*since, ctx.now)),
                    None => words.offline().to_string(),
                };
                spans.push(Span::styled(format!("  {offline}"), ctx.look.text(Role::Faint)));
            }
            MachineState::NoAnswer { machine, .. } => {
                spans.push(Span::styled("▲ ", ctx.look.text(Role::Caution)));
                spans.push(Span::styled(machine.to_string(), ctx.look.text(Role::Muted)));
                spans.push(Span::styled(format!("  {}", words.no_answer()), ctx.look.text(Role::Caution)));
            }
        }
    }
    Line::from(spans)
}

/// This machine, and how its link to the hub stands.
pub fn link_line(ctx: &Ctx, nodes: Option<&NodesReply>) -> Line<'static> {
    let words = &ctx.words;
    let Some(nodes) = nodes else { return Line::default() };
    // The hubs taken together: online when one is, and why not otherwise.
    let links: Vec<&LinkState> = nodes.hubs.iter().map(|hub| &hub.link).collect();
    let (state, role) = if links.is_empty() {
        (words.alone().to_string(), Role::Muted)
    } else if links.iter().any(|link| matches!(link, LinkState::Online { .. })) {
        (words.hub_online().to_string(), Role::Muted)
    } else if links.iter().any(|link| matches!(link, LinkState::Connecting { .. })) {
        (words.connecting().to_string(), Role::Caution)
    } else {
        (words.hub_stopped().to_string(), Role::Wrong)
    };
    let mut spans = vec![Span::styled(nodes.machine.to_string(), ctx.look.text(Role::Muted))];
    if !links.is_empty() {
        let mut machines: Vec<&fastctx_types::MachineName> =
            nodes.hubs.iter().flat_map(|hub| hub.machines.iter().map(|machine| &machine.name)).collect();
        machines.sort();
        machines.dedup();
        spans.push(Span::styled(format!("  ·  {}", words.machine_count(machines.len())), ctx.look.text(Role::Faint)));
    }
    spans.push(Span::styled(format!("  ·  {state}"), ctx.look.text(role)));
    Line::from(spans)
}
