//! The World's machines (`world-network.md`): how this one is linked to the
//! hub, each machine and what it is like, and bringing a machine in or
//! sending one away.

use crossterm::event::{KeyCode, KeyEvent};
use fastctx_types::world::{LinkState, NodesReply};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::{List, Page, Sheet, quiet, sheet, title, wrong};
use crate::app::{Act, Ctx, Data, Done, Job};
use crate::design::Role;
use crate::pages::form;
use crate::paint::{self, Row};
use crate::sessions::{Report, Standing, Status};
use crate::text;

#[derive(Default)]
pub struct MachinesPage {
    nodes: Option<NodesReply>,
    failed: Option<String>,
    had_at: Option<Timestamp>,
    list: List,
    /// Whether the user's own sessions here have the World's tools.
    sessions: Option<Result<Status, String>>,
    /// What the latest apply or unapply came to, or that one is under way.
    outcome: Option<Outcome>,
}

enum Outcome {
    Applying,
    Unapplying,
    Applied(Report),
    Unapplied(Report),
    NeedsRaise { current: i64, requested: u64 },
    Failed(String),
}

impl MachinesPage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(nodes: NodesReply) -> Self {
        Self { nodes: Some(nodes), had_at: Some(Timestamp::now()), ..Self::default() }
    }

    pub fn data(&mut self, data: Data) -> Act {
        match data {
            Data::Nodes(Ok(nodes)) => {
                self.list.clamp(nodes.machines.len());
                self.nodes = Some(nodes);
                self.failed = None;
                self.had_at = Some(Timestamp::now());
            }
            Data::Nodes(Err(why)) => self.failed = Some(why),
            Data::Sessions(sessions) => self.sessions = Some(sessions),
            Data::Done { job: Job::Apply { .. } | Job::Unapply, result } => {
                self.outcome = Some(match result {
                    Ok(Done::Applied(report)) => Outcome::Applied(report),
                    Ok(Done::Unapplied(report)) => Outcome::Unapplied(report),
                    Ok(Done::NeedsRaise { current, requested }) => Outcome::NeedsRaise { current, requested },
                    Ok(_) => return Act::Stay,
                    Err(why) => Outcome::Failed(why),
                });
            }
            _ => {}
        }
        Act::Stay
    }

    fn busy(&self) -> bool {
        matches!(self.outcome, Some(Outcome::Applying | Outcome::Unapplying))
    }

    fn any_in(&self) -> bool {
        let taken = |standing: Standing| matches!(standing, Standing::In | Standing::Old);
        matches!(&self.sessions, Some(Ok(status)) if taken(status.codex) || taken(status.claude))
    }

    fn any_installed(&self) -> bool {
        matches!(&self.sessions, Some(Ok(status)) if status.codex != Standing::Absent || status.claude != Standing::Absent)
    }

    pub fn key(&mut self, _ctx: &Ctx, key: KeyEvent) -> Act {
        let count = self.nodes.as_ref().map_or(0, |nodes| nodes.machines.len());
        if self.list.key(&key, count) {
            return Act::Stay;
        }
        let here = self.nodes.as_ref().map(|nodes| nodes.machine.clone());
        let selected = self.nodes.as_ref().and_then(|nodes| nodes.machines.get(self.list.selected));
        match key.code {
            KeyCode::Char('i') => Act::Push(Page::Form(Box::new(form::FormPage::invite()))),
            KeyCode::Char('j') => Act::Push(Page::Form(Box::new(form::FormPage::join()))),
            KeyCode::Char('r') => match selected {
                Some(node) if Some(&node.name) != here.as_ref() && node.revoked.is_none() => {
                    Act::Push(Page::Form(Box::new(form::FormPage::revoke(node.name.clone()))))
                }
                _ => Act::Stay,
            },
            KeyCode::Char('l') if self.nodes.as_ref().is_some_and(|nodes| !matches!(nodes.link, LinkState::Alone)) => {
                Act::Push(Page::Form(Box::new(form::FormPage::leave())))
            }
            KeyCode::Char('u') if !self.busy() && self.any_installed() => {
                self.outcome = Some(Outcome::Applying);
                Act::Do(Job::Apply { raise: false })
            }
            KeyCode::Char('U') if matches!(self.outcome, Some(Outcome::NeedsRaise { .. })) => {
                self.outcome = Some(Outcome::Applying);
                Act::Do(Job::Apply { raise: true })
            }
            KeyCode::Char('x') if !self.busy() && self.any_in() => {
                self.outcome = Some(Outcome::Unapplying);
                Act::Do(Job::Unapply)
            }
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => Act::Back,
            _ => Act::Stay,
        }
    }

    /// The line on the user's own sessions, and what the latest apply or
    /// unapply came to.
    fn sessions_rows(&self, ctx: &Ctx, width: usize) -> Vec<Row> {
        let words = &ctx.words;
        let look = &ctx.look;
        let mut rows = Vec::new();
        let standing = match &self.sessions {
            None => quiet(ctx, words.asking()),
            Some(Err(why)) => wrong(ctx, text::first_line(why)),
            Some(Ok(status)) => {
                let role = |standing: Standing| match standing {
                    Standing::In => Role::Alive,
                    Standing::Old => Role::Caution,
                    Standing::Out => Role::Muted,
                    Standing::Absent => Role::Faint,
                };
                Line::from(vec![
                    Span::styled(words.standing("Codex", status.codex), look.text(role(status.codex))),
                    Span::styled("  ·  ", look.text(Role::Faint)),
                    Span::styled(words.standing("Claude Code", status.claude), look.text(role(status.claude))),
                ])
            }
        };
        rows.push(Row::new(Span::styled(words.own_sessions(), look.text(Role::Muted))).right(standing));
        // What an old fastctx left here, and where it went: the user decides
        // whether to delete it (`world-upgrade.md`).
        if let Some(Ok(status)) = &self.sessions
            && let Some(legacy) = &status.legacy
            && (legacy.archived > 0 || legacy.tidied > 0 || legacy.waiting > 0)
        {
            let place = legacy.archives.last().unwrap_or(&legacy.record);
            let said = words.legacy_cleaned(legacy.tidied, legacy.archived, &shown_path(place), legacy.waiting);
            for line in text::wrap_plain(&said, look.text(Role::Faint), width) {
                rows.push(Row::new(line));
            }
        }
        let (said, role) = match &self.outcome {
            None => return rows,
            Some(Outcome::Applying) => (words.applying().to_string(), Role::Faint),
            Some(Outcome::Unapplying) => (words.unapplying().to_string(), Role::Faint),
            Some(Outcome::Applied(report)) => (words.applied(report.changed.len(), report.commands.len()), Role::Muted),
            Some(Outcome::Unapplied(report)) => {
                (words.unapplied(report.changed.len(), report.commands.len()), Role::Muted)
            }
            Some(Outcome::NeedsRaise { current, requested }) => {
                (words.needs_raise(*current, *requested), Role::Caution)
            }
            Some(Outcome::Failed(why)) => (why.clone(), Role::Wrong),
        };
        for line in text::wrap_plain(&said, look.text(role), width) {
            rows.push(Row::new(line));
        }
        if let Some(Outcome::Applied(report) | Outcome::Unapplied(report)) = &self.outcome {
            for notice in &report.notices {
                for line in text::wrap_plain(
                    &words.left_alone(&notice.location, &notice.reason),
                    look.text(Role::Caution),
                    width,
                ) {
                    rows.push(Row::new(line));
                }
            }
            for leftover in &report.leftovers {
                for line in text::wrap_plain(leftover, look.text(Role::Faint), width) {
                    rows.push(Row::new(line));
                }
            }
        }
        rows
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect) {
        let words = &ctx.words;
        let look = &ctx.look;
        let width = paint::column(area).width as usize;
        let mut rows = Vec::new();
        let mut alone = false;
        if let Some(nodes) = &self.nodes {
            paint::open_section(&mut rows, paint::section(words.this_machine_link(), look));
            let (line, role) = match &nodes.link {
                LinkState::Alone => {
                    alone = true;
                    (words.link_alone(), Role::Muted)
                }
                LinkState::Connecting { since, last_error } => (
                    words.link_connecting(
                        &crate::words::clock(*since, ctx.now),
                        last_error.as_deref().map(text::first_line).as_deref(),
                    ),
                    Role::Caution,
                ),
                LinkState::Online { since, hub, relay, route } => (
                    words.link_online(hub, relay.as_ref(), route, &crate::words::clock(*since, ctx.now)),
                    Role::Muted,
                ),
                LinkState::Stopped { reason } => (words.link_stopped(&text::first_line(reason)), Role::Wrong),
            };
            for line in text::wrap_plain(&line, look.text(role), width) {
                rows.push(Row::new(line));
            }
            rows.extend(self.sessions_rows(ctx, width));
            rows.push(Row::blank());
            paint::open_section(
                &mut rows,
                paint::section(words.machines(), look).right(quiet(ctx, words.machine_count(nodes.machines.len()))),
            );
            for (item, node) in nodes.machines.iter().enumerate() {
                let (mark, since) = match (&node.revoked, node.online) {
                    (Some(_), _) => (Span::styled("×", look.text(Role::Wrong)), String::new()),
                    (None, Some(true)) => (
                        Span::styled("●", look.text(Role::Alive)),
                        node.since
                            .map(|since| words.online_since(&crate::words::clock(since, ctx.now)))
                            .unwrap_or_default(),
                    ),
                    (None, Some(false)) => (
                        Span::styled("○", look.text(Role::Faint)),
                        node.since
                            .map(|since| words.offline_since(&crate::words::clock(since, ctx.now)))
                            .unwrap_or_else(|| words.offline().to_string()),
                    ),
                    (None, None) => (Span::styled("·", look.text(Role::Muted)), String::new()),
                };
                let mut left = vec![mark, Span::raw("  "), title(ctx, node.name.to_string())];
                if node.name == nodes.machine {
                    left.push(Span::styled(format!("   {}", words.this_machine()), look.text(Role::Faint)));
                }
                rows.push(Row::new(Line::from(left)).right(quiet(ctx, since)).item(item));
                let second = match (&node.revoked, &node.facts) {
                    (Some(revoked), _) => Some(Span::styled(
                        words.revoked(revoked.by.as_str(), &crate::words::clock(revoked.at, ctx.now), &revoked.reason),
                        look.text(Role::Wrong),
                    )),
                    (None, Ok(facts)) => Some(Span::styled(
                        format!("{} {}  ·  home {}  ·  fastctx {}", facts.os, facts.arch, facts.home, facts.version),
                        look.text(Role::Faint),
                    )),
                    // The first row already says it is offline, and since when.
                    (None, Err(_)) if node.online == Some(false) => None,
                    (None, Err(why)) => {
                        Some(Span::styled(words.not_asked(&text::first_line(why)), look.text(Role::Caution)))
                    }
                };
                if let Some(second) = second {
                    rows.push(Row::new(Line::from(vec![Span::raw("   "), second])).item(item));
                }
                rows.push(Row::blank());
            }
        }
        let note = match (&self.failed, self.had_at) {
            (Some(why), Some(at)) => Some(wrong(
                ctx,
                format!("{}  ·  {}", text::first_line(why), words.as_of(&crate::words::clock(at, ctx.now))),
            )),
            (Some(why), None) => Some(wrong(ctx, text::first_line(why))),
            (None, None) => Some(quiet(ctx, words.asking())),
            (None, Some(_)) => None,
        };
        let mut keys = vec![("i", words.key_invite()), ("j", words.key_join())];
        if matches!(self.outcome, Some(Outcome::NeedsRaise { .. })) {
            keys.push(("U", words.key_raise()));
        } else if self.any_installed() && !self.busy() {
            let old = matches!(&self.sessions, Some(Ok(status)) if status.codex == Standing::Old);
            keys.push(("u", if old { words.key_replace_old() } else { words.key_apply() }));
        }
        if self.any_in() && !self.busy() {
            keys.push(("x", words.key_unapply()));
        }
        if !alone {
            keys.push(("r", words.key_revoke()));
            keys.push(("l", words.key_leave()));
        }
        keys.push(("Esc", words.key_back()));
        let page = Sheet {
            crumbs: vec![words.world().to_string(), words.machines().to_string()],
            right: Line::default(),
            rows,
            keys,
            note,
        };
        sheet(ctx, frame, area, &page, &mut self.list);
    }
}

/// A path as the TUI writes paths: with forward slashes on every system.
fn shown_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    if cfg!(windows) { text.replace('\\', "/") } else { text }
}
