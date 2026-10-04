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
use crate::app::{Act, Ctx, Data};
use crate::design::Role;
use crate::pages::form;
use crate::paint::{self, Row};
use crate::text;

#[derive(Default)]
pub struct MachinesPage {
    nodes: Option<NodesReply>,
    failed: Option<String>,
    had_at: Option<Timestamp>,
    list: List,
}

impl MachinesPage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(nodes: NodesReply) -> Self {
        Self { nodes: Some(nodes), had_at: Some(Timestamp::now()), ..Self::default() }
    }

    pub fn data(&mut self, data: Data) -> Act {
        if let Data::Nodes(nodes) = data {
            match nodes {
                Ok(nodes) => {
                    self.list.clamp(nodes.machines.len());
                    self.nodes = Some(nodes);
                    self.failed = None;
                    self.had_at = Some(Timestamp::now());
                }
                Err(why) => self.failed = Some(why),
            }
        }
        Act::Stay
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
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => Act::Back,
            _ => Act::Stay,
        }
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect) {
        let words = &ctx.words;
        let look = &ctx.look;
        let width = paint::column(area).width as usize;
        let mut rows = Vec::new();
        let mut alone = false;
        if let Some(nodes) = &self.nodes {
            rows.push(paint::section(words.this_machine_link(), look));
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
                LinkState::Online { since, hub, route } => {
                    (words.link_online(hub, route, &crate::words::clock(*since, ctx.now)), Role::Muted)
                }
                LinkState::Stopped { reason } => (words.link_stopped(&text::first_line(reason)), Role::Wrong),
            };
            for line in text::wrap_plain(&line, look.text(role), width) {
                rows.push(Row::new(line));
            }
            rows.push(Row::blank());
            rows.push(
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
                    (Some(revoked), _) => Span::styled(
                        words.revoked(revoked.by.as_str(), &crate::words::clock(revoked.at, ctx.now), &revoked.reason),
                        look.text(Role::Wrong),
                    ),
                    (None, Ok(facts)) => Span::styled(
                        format!("{} {}  ·  home {}  ·  fastctx {}", facts.os, facts.arch, facts.home, facts.version),
                        look.text(Role::Faint),
                    ),
                    (None, Err(why)) => Span::styled(words.not_asked(&text::first_line(why)), look.text(Role::Caution)),
                };
                rows.push(Row::new(Line::from(vec![Span::raw("   "), second])).item(item));
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
