//! A session as a conversation (`world-gui.md`, 对话): words said to the
//! agent, what it writes, each tool it uses with where and how it ended,
//! how each turn ended, and what its host said about the session. It reads
//! the session's events and nothing else; what it shows of words the user
//! is sending comes from what the World answered.
//!
//! What came before arrives as history and may repeat in what comes live;
//! the same words in the same turn, the same message or the same tool use
//! are shown once.

use std::cell::RefCell;

use fastctx_types::MachineName;
use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, Health, SessionEvent, Speaker, TurnEnd, TurnHow};
use jiff::Timestamp;
use ratatui::text::{Line, Span};

use crate::app::Ctx;
use crate::design::Role;
use crate::paint;
use crate::{markdown, text, who};

pub struct Transcript {
    items: Vec<Item>,
    next_local: u64,
    /// The machine this TUI runs on: the user's words typed here need no
    /// name above them.
    here: Option<MachineName>,
}

enum Item {
    Heard { from: Option<Speaker>, text: String, turn: Option<String>, sending: Option<Sending> },
    Message { item: String, turn: String, text: String, writing: bool, laid: Laid },
    Tool { item: String, turn: String, tool: String, detail: String, at: Timestamp, ended: Option<(Timestamp, Use)> },
    Ended { turn: String, end: TurnEnd, at: Timestamp },
    Health { health: Health },
    Note { text: String, role: Role },
}

/// What the agent wrote, laid out for the width it was last drawn at:
/// its Markdown is read again only when the text or the width changes.
#[derive(Default)]
struct Laid(RefCell<Option<(usize, Vec<Line<'static>>)>>);

impl Laid {
    fn lines(&self, ctx: &Ctx, text: &str, width: usize) -> Vec<Line<'static>> {
        let mut laid = self.0.borrow_mut();
        match &*laid {
            Some((at, lines)) if *at == width => lines.clone(),
            _ => {
                let lines = markdown::render(text, width, &ctx.look);
                *laid = Some((width, lines.clone()));
                lines
            }
        }
    }
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
    pub fn new(here: Option<MachineName>) -> Self {
        Self { items: Vec::new(), next_local: 0, here }
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
                    self.items.push(Item::Heard { from, text, turn, sending: None });
                }
            }
            Happened::Writing { turn, item, delta } => match self.message(&item) {
                Some(Item::Message { text, laid, .. }) => {
                    text.push_str(&delta);
                    *laid = Laid::default();
                }
                _ => self.items.push(Item::Message { item, turn, text: delta, writing: true, laid: Laid::default() }),
            },
            Happened::Said { turn, item, text } => match self.message(&item) {
                Some(Item::Message { text: shown, writing, laid, .. }) => {
                    *shown = text;
                    *writing = false;
                    *laid = Laid::default();
                }
                _ => self.items.push(Item::Message { item, turn, text, writing: false, laid: Laid::default() }),
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
                self.items.push(Item::Ended { turn, end, at });
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
    pub fn sending(&mut self, text: String, here: Option<MachineName>) -> u64 {
        let local = self.next_local;
        self.next_local += 1;
        let from = here.map(|machine| Speaker::User { machine });
        self.items.push(Item::Heard {
            from,
            text,
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

    /// The conversation laid out in `width` cells. Words that went in sit
    /// on a raised block, named only when they are not the user's own from
    /// this machine; what the agent writes sits on the ground under no name,
    /// since the page says whose session it is; its tools are quiet rows; a
    /// turn closes with how it ended and when.
    pub fn lines(&self, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
        let look = &ctx.look;
        let mut out: Vec<Line<'static>> = Vec::new();
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
            let is_tool = matches!(item, Item::Tool { .. });
            match item {
                Item::Heard { from, text, sending, .. } => {
                    blank(&mut out);
                    if let Some(label) = self.speaker(ctx, from.as_ref(), sending.is_some()) {
                        out.push(Line::from(Span::styled(label, look.text(Role::Muted))));
                    }
                    let role = if matches!(from, Some(Speaker::World)) { Role::Muted } else { Role::Text };
                    out.extend(block(ctx, text, role, width));
                    if let Some(sending) = sending {
                        out.extend(sent(ctx, &sending.state, width));
                    }
                }
                Item::Message { text, writing, laid, .. } => {
                    blank(&mut out);
                    let mut lines = laid.lines(ctx, text, width);
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
                }
                Item::Tool { tool, detail, at, ended, .. } => {
                    if !last_was_tool {
                        blank(&mut out);
                    }
                    out.push(tool_line(ctx, tool, tool_width, detail, *at, *ended, width));
                }
                Item::Ended { end, at, .. } => {
                    blank(&mut out);
                    out.push(ended(ctx, end, *at, width));
                }
                Item::Health { health, .. } => {
                    out.push(Line::from(Span::styled(format!("▲ {}", health.text), look.text(Role::Caution))));
                }
                Item::Note { text, role } => {
                    blank(&mut out);
                    out.extend(text::wrap_plain(text, look.text(*role), width));
                }
            }
            last_was_tool = is_tool;
        }
        out
    }

    /// Who words that went in came from, when it needs saying: not for the
    /// user's own, typed on this machine.
    fn speaker(&self, ctx: &Ctx, from: Option<&Speaker>, sending: bool) -> Option<String> {
        match from {
            None if sending => None,
            Some(Speaker::User { machine }) if self.here.as_ref() == Some(machine) => None,
            Some(Speaker::User { machine }) => Some(format!("{} · {machine}", ctx.words.you())),
            Some(Speaker::Resident { address }) => {
                Some(format!("{}  {}", who::address_name(ctx, address), address.resident))
            }
            Some(Speaker::World) => Some(ctx.words.the_world().to_string()),
            None => Some(ctx.words.heard().to_string()),
        }
    }
}

/// One blank line between blocks, never two, never first.
fn blank(out: &mut Vec<Line<'static>>) {
    if out.last().is_some_and(|line| text::line_width(line) > 0) {
        out.push(Line::default());
    }
}

/// Words on a raised block that fits them, with room inside: a row above
/// and below, two cells at each side. Where the terminal has no colour for
/// a raised ground, a rule in the gutter marks the words instead.
fn block(ctx: &Ctx, words: &str, role: Role, width: usize) -> Vec<Line<'static>> {
    let look = &ctx.look;
    if look.color(Role::Raised).is_none() {
        let rule = Span::styled("│ ", look.text(Role::Faint));
        return text::wrap_plain(words, look.text(role), width.saturating_sub(2))
            .into_iter()
            .map(|mut line| {
                line.spans.insert(0, rule.clone());
                line
            })
            .collect();
    }
    let lines = text::wrap_plain(words, look.raised(role), width.saturating_sub(4));
    let inside = lines.iter().map(text::line_width).max().unwrap_or(0);
    let band = (inside + 4).min(width);
    let fill = |cells: usize| Span::styled(" ".repeat(cells), look.raised(Role::Text));
    let mut out = vec![Line::from(fill(band))];
    for mut line in lines {
        let used = text::line_width(&line);
        line.spans.insert(0, fill(2));
        line.spans.push(fill(band.saturating_sub(used + 2)));
        out.push(line);
    }
    out.push(Line::from(fill(band)));
    out
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

/// How a turn ended, in a quiet line, with when on the right; louder when
/// it did not complete.
fn ended(ctx: &Ctx, end: &TurnEnd, at: Timestamp, width: usize) -> Line<'static> {
    let words = &ctx.words;
    let took = end.duration_ms.map(|ms| words.duration(ms / 1000)).unwrap_or_default();
    let reason = end.reason.as_deref().map(text::first_line);
    let (said, role) = match &end.how {
        TurnHow::Completed => (words.turn_completed(&took), Role::Faint),
        TurnHow::Failed => (words.turn_failed(reason.as_deref()), Role::Wrong),
        TurnHow::Interrupted => (words.turn_interrupted(reason.as_deref()), Role::Caution),
        TurnHow::Other(how) => (words.turn_ended_as(how), Role::Muted),
    };
    let clock = crate::words::clock(at, ctx.now);
    let room = width.saturating_sub(text::width(&clock) + 2);
    let mut line = paint::fit_line(&Line::from(Span::styled(said, ctx.look.text(role))), room);
    let used = text::line_width(&line);
    line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used + text::width(&clock)))));
    line.spans.push(Span::styled(clock, ctx.look.text(Role::Faint)));
    line
}
