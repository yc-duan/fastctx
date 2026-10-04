//! A session as a conversation (`world-gui.md`, 对话): words said to the
//! agent, what it writes, each tool it uses with where and how it ended,
//! how each turn ended, and what its host said about the session. It reads
//! the session's events and nothing else; what it shows of words the user
//! is sending comes from what the World answered.
//!
//! What came before arrives as history and may repeat in what comes live;
//! the same words in the same turn, the same message or the same tool use
//! are shown once.

use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, Health, SessionEvent, Speaker, TurnEnd, TurnHow};
use fastctx_types::{Address, MachineName};
use jiff::Timestamp;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::app::Ctx;
use crate::design::Role;
use crate::paint;
use crate::{text, who};

pub struct Transcript {
    /// How the agent is named above what it writes: "Codex".
    agent: String,
    items: Vec<Item>,
    next_local: u64,
}

enum Item {
    Heard { from: Option<Speaker>, text: String, at: Timestamp, turn: Option<String>, sending: Option<Sending> },
    Message { item: String, turn: String, text: String, at: Timestamp, writing: bool },
    Tool { item: String, turn: String, tool: String, detail: String, at: Timestamp, ended: Option<(Timestamp, Use)> },
    Ended { turn: String, end: TurnEnd },
    Health { health: Health },
    Note { text: String, role: Role },
}

/// Words the user is sending from here, until the session says it heard
/// them.
struct Sending {
    local: u64,
    state: SendState,
}

enum SendState {
    Going,
    Landed(Landed, MachineName),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Done,
    Failed,
    /// Its turn ended before it did.
    Cut,
}

impl Transcript {
    pub fn new(agent: String) -> Self {
        Self { agent, items: Vec::new(), next_local: 0 }
    }

    /// What came before, replacing what was shown; words being sent from
    /// here stay after it until the session says it heard them.
    pub fn history(&mut self, events: Vec<SessionEvent>) {
        let mut sending: Vec<Item> = Vec::new();
        for item in self.items.drain(..) {
            if matches!(item, Item::Heard { sending: Some(_), .. }) {
                sending.push(item);
            }
        }
        for event in events {
            self.event(event);
        }
        for item in sending {
            if let Item::Heard { text, .. } = &item
                && self.heard_already(text)
            {
                continue;
            }
            self.items.push(item);
        }
    }

    fn heard_already(&self, said: &str) -> bool {
        self.items.iter().any(|item| matches!(item, Item::Heard { text, sending: None, .. } if text == said))
    }

    pub fn event(&mut self, event: SessionEvent) {
        let at = event.at;
        match event.happened {
            Happened::TurnStarted { .. } => {}
            Happened::Heard { turn, from, text } => {
                // Words sent from here are now heard: they keep their place.
                if matches!(from, Some(Speaker::User { .. }))
                    && let Some(Item::Heard { sending, turn: heard_turn, from: heard_from, .. }) = self
                        .items
                        .iter_mut()
                        .find(|item| matches!(item, Item::Heard { text: said, sending: Some(_), .. } if *said == text))
                {
                    *sending = None;
                    *heard_turn = turn;
                    *heard_from = from;
                    return;
                }
                let known = self.items.iter().any(|item| {
                    matches!(item, Item::Heard { text: said, turn: heard, .. } if *said == text && turn.is_some() && *heard == turn)
                });
                if !known {
                    self.items.push(Item::Heard { from, text, at, turn, sending: None });
                }
            }
            Happened::Writing { turn, item, delta } => match self.message(&item) {
                Some(Item::Message { text, .. }) => text.push_str(&delta),
                _ => self.items.push(Item::Message { item, turn, text: delta, at, writing: true }),
            },
            Happened::Said { turn, item, text } => match self.message(&item) {
                Some(Item::Message { text: shown, writing, .. }) => {
                    *shown = text;
                    *writing = false;
                }
                _ => self.items.push(Item::Message { item, turn, text, at, writing: false }),
            },
            Happened::ToolStarted { turn, item, tool, detail } => {
                let known = !item.is_empty()
                    && self.items.iter().any(|shown| matches!(shown, Item::Tool { item: id, .. } if *id == item));
                if !known {
                    self.items.push(Item::Tool { item, turn, tool, detail, at, ended: None });
                }
            }
            Happened::ToolEnded { turn, item, tool, detail, failed } => {
                let how = if failed { Use::Failed } else { Use::Done };
                let found = self.items.iter_mut().rev().find(|shown| match shown {
                    Item::Tool { item: id, .. } if !item.is_empty() => *id == item,
                    Item::Tool { turn: t, tool: name, detail: d, ended: None, .. } => {
                        *t == turn && *name == tool && *d == detail
                    }
                    _ => false,
                });
                match found {
                    Some(Item::Tool { ended, .. }) => *ended = Some((at, how)),
                    _ => self.items.push(Item::Tool { item, turn, tool, detail, at, ended: Some((at, how)) }),
                }
            }
            Happened::TurnEnded { turn, end } => {
                if self.items.iter().any(|item| matches!(item, Item::Ended { turn: ended, .. } if *ended == turn)) {
                    return;
                }
                for item in &mut self.items {
                    match item {
                        Item::Tool { turn: t, ended: ended @ None, .. } if *t == turn => *ended = Some((at, Use::Cut)),
                        Item::Message { turn: t, writing, .. } if *t == turn => *writing = false,
                        _ => {}
                    }
                }
                self.items.push(Item::Ended { turn, end });
            }
            Happened::Health { health, .. } => {
                let repeat = matches!(self.items.last(), Some(Item::Health { health: last, .. }) if *last == health);
                if !repeat {
                    self.items.push(Item::Health { health });
                }
            }
        }
    }

    fn message(&mut self, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|item| matches!(item, Item::Message { item, .. } if item == id))
    }

    /// Words the user is sending; returns the number they are known by
    /// until the World answers.
    pub fn sending(&mut self, text: String, here: Option<MachineName>, at: Timestamp) -> u64 {
        let local = self.next_local;
        self.next_local += 1;
        let from = here.map(|machine| Speaker::User { machine });
        self.items.push(Item::Heard {
            from,
            text,
            at,
            turn: None,
            sending: Some(Sending { local, state: SendState::Going }),
        });
        local
    }

    /// What became of words sent from here.
    pub fn landed(&mut self, local: u64, landed: Landed, machine: MachineName) {
        if let Some(sending) = self.sending_mut(local) {
            sending.state = SendState::Landed(landed, machine);
        }
    }

    pub fn failed(&mut self, local: u64, why: String) {
        if let Some(sending) = self.sending_mut(local) {
            sending.state = SendState::Failed(why);
        }
    }

    fn sending_mut(&mut self, local: u64) -> Option<&mut Sending> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Heard { sending: Some(sending), .. } if sending.local == local => Some(sending),
            _ => None,
        })
    }

    pub fn note(&mut self, text: String, role: Role) {
        self.items.push(Item::Note { text, role });
    }

    /// Whether anything shown is happening now.
    pub fn moving(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(item, Item::Tool { ended: None, .. } | Item::Message { writing: true, .. })
                || matches!(item, Item::Heard { sending: Some(Sending { state: SendState::Going, .. }), .. })
        })
    }

    /// The conversation laid out in `width` cells.
    pub fn lines(&self, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
        let words = &ctx.words;
        let look = &ctx.look;
        let mut out: Vec<Line<'static>> = Vec::new();
        // The turn whose agent is already named above what follows.
        let mut named: Option<&str> = None;
        let mut last_was_tool = false;
        let tool_width = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool { tool, .. } => Some(text::width(short_tool(tool))),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        for item in &self.items {
            match item {
                Item::Heard { from, text, at, sending, .. } => {
                    blank(&mut out);
                    named = None;
                    out.push(label(ctx, speaker(ctx, from.as_ref(), sending.is_some()), *at, width));
                    let role = if matches!(from, Some(Speaker::World)) { Role::Muted } else { Role::Text };
                    out.extend(text::wrap_plain(text, look.text(role), width));
                    if let Some(sending) = sending {
                        out.extend(sent(ctx, &sending.state, width));
                    }
                }
                Item::Message { turn, text, at, writing, .. } => {
                    if named != Some(turn.as_str()) {
                        blank(&mut out);
                        out.push(label(ctx, self.agent.clone(), *at, width));
                        named = Some(turn.as_str());
                    } else if last_was_tool {
                        blank(&mut out);
                    }
                    let mut lines = message_lines(ctx, text, width);
                    if *writing {
                        let breath = look.faded(Role::Accent, ctx.breath);
                        match lines.last_mut() {
                            Some(line) if text::line_width(line) + 2 <= width => {
                                line.spans.push(Span::styled(" ▍", breath))
                            }
                            _ => lines.push(Line::from(Span::styled("▍", breath))),
                        }
                    }
                    out.extend(lines);
                    last_was_tool = false;
                    continue;
                }
                Item::Tool { turn, tool, detail, at, ended, .. } => {
                    if named != Some(turn.as_str()) {
                        blank(&mut out);
                        out.push(label(ctx, self.agent.clone(), *at, width));
                        named = Some(turn.as_str());
                    } else if !last_was_tool {
                        blank(&mut out);
                    }
                    out.push(tool_line(ctx, tool, tool_width, detail, *at, *ended, width));
                    last_was_tool = true;
                    continue;
                }
                Item::Ended { end, .. } => {
                    out.push(Line::default());
                    out.push(ended(ctx, end));
                    named = None;
                }
                Item::Health { health, .. } => {
                    out.push(Line::from(Span::styled(format!("▲ {}", health.text), look.text(Role::Caution))));
                }
                Item::Note { text, role } => {
                    blank(&mut out);
                    out.extend(text::wrap_plain(text, look.text(*role), width));
                }
            }
            last_was_tool = false;
        }
        let _ = words;
        out
    }
}

/// One blank line between blocks, never two, never first.
fn blank(out: &mut Vec<Line<'static>>) {
    if out.last().is_some_and(|line| text::line_width(line) > 0) {
        out.push(Line::default());
    }
}

/// Who said something, as a line above it, with the time on the right.
fn label(ctx: &Ctx, who: String, at: Timestamp, width: usize) -> Line<'static> {
    let clock = crate::words::clock(at, ctx.now);
    let who_width = text::width(&who);
    let gap = width.saturating_sub(who_width + text::width(&clock)).max(2);
    Line::from(vec![
        Span::styled(who, ctx.look.text(Role::Muted).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(gap)),
        Span::styled(clock, ctx.look.text(Role::Faint)),
    ])
}

fn speaker(ctx: &Ctx, from: Option<&Speaker>, sending: bool) -> String {
    match from {
        None if sending => ctx.words.you().to_string(),
        Some(Speaker::User { machine }) => format!("{} · {machine}", ctx.words.you()),
        Some(Speaker::Resident { address }) => resident(ctx, address),
        Some(Speaker::World) => ctx.words.the_world().to_string(),
        None => ctx.words.heard().to_string(),
    }
}

fn resident(ctx: &Ctx, address: &Address) -> String {
    format!("{}  {}", who::address_name(ctx, address), address.resident)
}

/// What the World said of words sent from here.
fn sent(ctx: &Ctx, state: &SendState, width: usize) -> Vec<Line<'static>> {
    let words = &ctx.words;
    let (text, role) = match state {
        SendState::Going => (words.sending().to_string(), Role::Faint),
        SendState::Failed(why) => (words.not_sent(&text::first_line(why)), Role::Wrong),
        SendState::Landed(landed, machine) => match landed {
            Landed::NewTurn { .. } => return Vec::new(),
            Landed::JoinedTurn { .. } => (words.joined_turn().to_string(), Role::Faint),
            Landed::Handed => (words.handed().to_string(), Role::Faint),
            Landed::Sent => (words.sent_to(machine.as_str()), Role::Faint),
            Landed::Waiting { why, answered } => {
                let mut text = match why {
                    Some(why) => words.waiting_because(why),
                    None => words.waiting_for_call().to_string(),
                };
                if !answered {
                    text.push_str(words.no_turn_end_known());
                }
                (text, Role::Caution)
            }
        },
    };
    text::wrap_plain(&text, ctx.look.text(role), width)
}

/// What the agent wrote. Markdown is laid out as text until its renderer
/// takes over.
fn message_lines(ctx: &Ctx, text: &str, width: usize) -> Vec<Line<'static>> {
    text::wrap_plain(text, ctx.look.text(Role::Text), width)
}

/// A tool as its host names it, without the server it came from.
fn short_tool(tool: &str) -> &str {
    let tail = tool.rsplit("__").next().unwrap_or(tool);
    tail.rsplit('.').next().unwrap_or(tail)
}

#[allow(clippy::too_many_arguments)]
fn tool_line(
    ctx: &Ctx,
    tool: &str,
    tool_width: usize,
    detail: &str,
    at: Timestamp,
    ended: Option<(Timestamp, Use)>,
    width: usize,
) -> Line<'static> {
    let look = &ctx.look;
    let (mark, detail_role) = match ended {
        None => (Span::styled("◌", look.faded(Role::Alive, ctx.breath)), Role::Text),
        Some((_, Use::Done)) => (Span::styled("✓", look.text(Role::Faint)), Role::Muted),
        Some((_, Use::Failed)) => (Span::styled("×", look.text(Role::Wrong)), Role::Muted),
        Some((_, Use::Cut)) => (Span::styled("·", look.text(Role::Faint)), Role::Faint),
    };
    let seconds = match ended {
        Some((end, _)) => end.as_second() - at.as_second(),
        None => ctx.now.as_second() - at.as_second(),
    };
    let took = if seconds >= 1 { ctx.words.duration(seconds as u64) } else { String::new() };
    let left = Line::from(vec![
        Span::raw("  "),
        mark,
        Span::raw("  "),
        Span::styled(format!("{:<tool_width$}", short_tool(tool)), look.text(Role::Faint)),
        Span::raw("  "),
        Span::styled(text::first_line(detail), look.text(detail_role)),
    ]);
    let room = width.saturating_sub(text::width(&took) + 2);
    let mut line = paint::fit_line(&left, room);
    let used = text::line_width(&line);
    if !took.is_empty() {
        line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used + text::width(&took)))));
        line.spans.push(Span::styled(took, look.text(Role::Faint)));
    }
    line
}

/// How a turn ended, in a quiet line; louder when it did not complete.
fn ended(ctx: &Ctx, end: &TurnEnd) -> Line<'static> {
    let words = &ctx.words;
    let took = end.duration_ms.map(|ms| words.duration(ms / 1000)).unwrap_or_default();
    let reason = end.reason.as_deref().map(text::first_line);
    let (text, role) = match &end.how {
        TurnHow::Completed => (words.turn_completed(&took), Role::Faint),
        TurnHow::Failed => (words.turn_failed(reason.as_deref()), Role::Wrong),
        TurnHow::Interrupted => (words.turn_interrupted(reason.as_deref()), Role::Caution),
        TurnHow::Other(how) => (words.turn_ended_as(how), Role::Muted),
    };
    Line::from(Span::styled(text, ctx.look.text(role)))
}
