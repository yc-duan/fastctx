//! A session as a conversation (`world-gui.md`, 对话): words said to the
//! agent, what it writes and what it shows of its thinking, the steps it
//! set out, each tool it uses with where and how it ended, how each turn
//! ended, and what its host said about the session. It reads the session's
//! events and nothing else; what it shows of words the user is sending
//! comes from what the World answered.
//!
//! What came before arrives as history and may repeat in what comes live;
//! the same words in the same turn and the same item are shown once.

use std::cell::RefCell;
use std::sync::Arc;

use fastctx_types::MachineName;
use fastctx_types::residents::Landed;
use fastctx_types::session::{
    Does, Happened, Health, Part, SessionEvent, Speaker, Step, StepStatus, Tool, ToolStatus, TurnEnd, TurnHow, What,
};
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
    /// How full the session's context is, as its host last said.
    context: Option<(u64, Option<u64>)>,
    title: Option<String>,
    /// Each tool's input and output shown under its row.
    details: bool,
    /// Its lines as last laid out, and for what width: drawn again and
    /// again while nothing changes, they are laid out once.
    drawn: RefCell<Option<(usize, Arc<Vec<Line<'static>>>)>>,
}

enum Item {
    Heard {
        from: Option<Speaker>,
        text: String,
        turn: Option<String>,
        sending: Option<Sending>,
    },
    Prose {
        item: String,
        turn: String,
        kind: Prose,
        text: String,
        writing: bool,
        laid: Laid,
        /// Thinking: how long it took, where the host said.
        took: Option<u64>,
    },
    Tool {
        item: String,
        turn: String,
        tool: Box<Tool>,
        at: Timestamp,
        ended: Option<(Timestamp, Use)>,
    },
    Steps {
        turn: String,
        steps: Vec<Step>,
        note: Option<String>,
    },
    Compaction {
        item: String,
        turn: String,
    },
    /// Something the host reported that the World has no name for.
    Other {
        item: String,
        turn: String,
        kind: String,
    },
    /// What one of the agent's own sub-agents is doing, as its host says.
    Activity {
        item: String,
        turn: String,
        text: String,
    },
    Ended {
        turn: String,
        end: TurnEnd,
    },
    Health {
        health: Health,
    },
    Note {
        text: String,
        role: Role,
    },
    /// Words the World handed over without learning whether a turn took
    /// them.
    Uncertain,
}

/// What the agent writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prose {
    Message,
    Thinking,
    Plan,
}

/// What the agent wrote, laid out for the width it was last drawn at:
/// its Markdown is read again only when the text or the width changes.
#[derive(Default)]
struct Laid {
    shown: RefCell<Option<(usize, Vec<Line<'static>>)>>,
    /// How its code was coloured, kept while the text grows.
    memo: RefCell<markdown::Memo>,
}

impl Laid {
    fn lines(&self, ctx: &Ctx, text: &str, width: usize) -> Vec<Line<'static>> {
        let mut shown = self.shown.borrow_mut();
        match &*shown {
            Some((at, lines)) if *at == width => lines.clone(),
            _ => {
                let lines = markdown::render_remembering(text, width, &ctx.look, &mut self.memo.borrow_mut());
                *shown = Some((width, lines.clone()));
                lines
            }
        }
    }

    /// The text changed: it is laid out again when next drawn.
    fn changed(&self) {
        self.shown.borrow_mut().take();
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
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Done,
    Failed,
    /// Refused before it ran.
    Declined,
    /// Its turn ended before it did.
    Cut,
    Unknown,
}

impl Transcript {
    pub fn new(here: Option<MachineName>) -> Self {
        Self {
            items: Vec::new(),
            next_local: 0,
            here,
            context: None,
            title: None,
            details: false,
            drawn: RefCell::new(None),
        }
    }

    /// Something shown changed: the lines are laid out again when next drawn.
    fn changed(&self) {
        self.drawn.borrow_mut().take();
    }

    /// Shows or hides what each tool was given and gave back.
    pub fn toggle_details(&mut self) {
        self.changed();
        self.details = !self.details;
    }

    pub fn details(&self) -> bool {
        self.details
    }

    /// Whether any tool was used, so there are details to show.
    pub fn has_tools(&self) -> bool {
        self.items.iter().any(|item| matches!(item, Item::Tool { .. }))
    }

    /// How much of its context the session holds and how much it can.
    pub fn context(&self) -> Option<(u64, Option<u64>)> {
        self.context
    }

    /// The title the host gave the session.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// What came before, replacing what was shown; words being sent from
    /// here stay after it until the session says it heard them.
    pub fn history(&mut self, events: Vec<SessionEvent>) {
        self.changed();
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
        self.changed();
        let at = event.at;
        match event.happened {
            Happened::TurnStarted { .. } | Happened::Realtime { .. } | Happened::RealtimeGrew { .. } => {}
            Happened::Heard { turn, from, text, .. } => {
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
            Happened::Began { turn, item } | Happened::Updated { turn, item } => self.item(turn, item, at, false),
            Happened::Finished { turn, item } => self.item(turn, item, at, true),
            Happened::Grew { turn, item, part, text: more, .. } => match (self.find(&turn, &item), part) {
                (Some(Item::Prose { text, laid, .. }), Part::Text) => {
                    text.push_str(&more);
                    laid.changed();
                }
                (Some(_), _) => {}
                // A message's first words can come before any word of it.
                (None, Part::Text) => self.items.push(Item::Prose {
                    item,
                    turn,
                    kind: Prose::Message,
                    text: more,
                    writing: true,
                    laid: Laid::default(),
                    took: None,
                }),
                (None, Part::Output) => {}
            },
            Happened::Steps { turn, steps, note } => {
                match self.items.iter_mut().rev().find(|item| matches!(item, Item::Steps { turn: t, .. } if *t == turn))
                {
                    Some(Item::Steps { steps: shown, note: said, .. }) => {
                        *shown = steps;
                        *said = note;
                    }
                    _ => self.items.push(Item::Steps { turn, steps, note }),
                }
            }
            Happened::Context { used, window } => self.context = Some((used, window)),
            Happened::Titled { title } => self.title = Some(title),
            Happened::TurnEnded { turn, end } => {
                if self.items.iter().any(|item| matches!(item, Item::Ended { turn: ended, .. } if *ended == turn)) {
                    return;
                }
                for item in &mut self.items {
                    match item {
                        Item::Tool { turn: t, ended: ended @ None, .. } if *t == turn => *ended = Some((at, Use::Cut)),
                        Item::Prose { turn: t, writing, .. } if *t == turn => *writing = false,
                        _ => {}
                    }
                }
                self.items.push(Item::Ended { turn, end });
            }
            Happened::DeliveryUncertain { .. } => {
                self.items.push(Item::Uncertain);
            }
            Happened::Health { health, .. } => self.health(health),
            Happened::Changed { change, .. } => {
                if let Some(health) = change.health() {
                    self.health(health);
                }
            }
        }
    }

    /// Something of the session's own health, shown once however often the
    /// host repeats it in a row.
    fn health(&mut self, health: Health) {
        let repeat = matches!(self.items.last(), Some(Item::Health { health: last, .. }) if *last == health);
        if !repeat {
            self.items.push(Item::Health { health });
        }
    }

    /// An item began or finished: shown where it first appeared, as it now
    /// stands.
    fn item(&mut self, turn: String, native: fastctx_types::session::Item, at: Timestamp, finished: bool) {
        let started_at = native.started_at.unwrap_or(at);
        let id = native.id;
        if let Some(draft) = &native.draft {
            self.renamed(&turn, draft, &id);
        }
        let what = native.what;
        let prose = |kind, text: String, took: Option<u64>| Item::Prose {
            item: id.clone(),
            turn: turn.clone(),
            kind,
            text,
            writing: !finished,
            laid: Laid::default(),
            took,
        };
        let fresh = match what {
            What::Message { text } => prose(Prose::Message, text, None),
            What::Thinking { text, duration_ms } => prose(Prose::Thinking, text, duration_ms),
            What::Plan { text } => prose(Prose::Plan, text, None),
            What::AgentActivity { path, activity, .. } => {
                Item::Activity { item: id.clone(), turn: turn.clone(), text: format!("{path}: {activity}") }
            }
            What::Tool(tool) => {
                let ended = match tool.status {
                    ToolStatus::Running => None,
                    ToolStatus::Unknown => Some((at, Use::Unknown)),
                    ToolStatus::Done => Some((at, Use::Done)),
                    ToolStatus::Failed => Some((at, Use::Failed)),
                    ToolStatus::Declined => Some((at, Use::Declined)),
                    ToolStatus::Cut => Some((at, Use::Cut)),
                };
                Item::Tool {
                    item: id.clone(),
                    turn: turn.clone(),
                    tool,
                    at: started_at,
                    ended: if finished { ended } else { None },
                }
            }
            // Shown once it is done: the context it leaves is what counts.
            What::Compaction if !finished => return,
            What::Compaction => Item::Compaction { item: id.clone(), turn: turn.clone() },
            // A completed item can remove a transient placeholder, for
            // example an image attempt that ended without an image.
            What::Hidden { .. } => {
                if !id.is_empty() {
                    self.items.retain(|shown| match shown {
                        Item::Prose { item, turn: own, .. }
                        | Item::Tool { item, turn: own, .. }
                        | Item::Compaction { item, turn: own }
                        | Item::Other { item, turn: own, .. }
                        | Item::Activity { item, turn: own, .. } => item != &id || own != &turn,
                        _ => true,
                    });
                }
                return;
            }
            // A kind the World does not know is shown for what the host
            // called it, once it is done.
            What::Other { .. } if !finished => return,
            What::Other { host_kind } => Item::Other { item: id.clone(), turn: turn.clone(), kind: host_kind.clone() },
        };
        if id.is_empty() {
            self.items.push(fresh);
            return;
        }
        match self.find(&turn, &id) {
            Some(shown) => {
                let began = match shown {
                    Item::Tool { at, .. } => Some(*at),
                    _ => None,
                };
                let empty_now = matches!(&fresh, Item::Prose { text, .. } if text.is_empty());
                match (shown, fresh) {
                    // A message still being written keeps what it has
                    // until its whole text comes.
                    (Item::Prose { writing: true, .. }, _) if !finished && empty_now => {}
                    (
                        Item::Prose { text, writing, laid, took, .. },
                        Item::Prose { text: whole, writing: still, took: given, .. },
                    ) => {
                        *text = whole;
                        *writing = still;
                        *took = given.or(*took);
                        laid.changed();
                    }
                    (shown @ Item::Tool { .. }, Item::Tool { item, turn, tool, ended, .. }) => {
                        *shown = Item::Tool { item, turn, tool, at: began.unwrap_or(at), ended };
                    }
                    (shown, fresh) => *shown = fresh,
                }
            }
            None => self.items.push(fresh),
        }
    }

    /// An item its host kept under a name of its own after it streamed as
    /// `draft`: the shown draft becomes that item where it stands; a copy
    /// already read from the host's record stays the one shown.
    fn renamed(&mut self, turn: &str, draft: &str, id: &str) {
        if self.find(turn, id).is_some() {
            self.items.retain(|item| !Self::named(item, turn, draft));
        } else if let Some(
            Item::Prose { item, .. }
            | Item::Tool { item, .. }
            | Item::Compaction { item, .. }
            | Item::Other { item, .. }
            | Item::Activity { item, .. },
        ) = self.find(turn, draft)
        {
            *item = id.to_string();
        }
    }

    fn named(item: &Item, turn: &str, id: &str) -> bool {
        match item {
            Item::Prose { item, turn: own, .. }
            | Item::Tool { item, turn: own, .. }
            | Item::Compaction { item, turn: own }
            | Item::Other { item, turn: own, .. }
            | Item::Activity { item, turn: own, .. } => item == id && own == turn,
            _ => false,
        }
    }

    fn find(&mut self, turn: &str, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|item| Self::named(item, turn, id))
    }

    /// Words the user is sending; returns the number they are known by
    /// until the World answers.
    pub fn sending(&mut self, text: String, here: Option<MachineName>) -> u64 {
        self.changed();
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
        self.changed();
        if let Some(sending) = self.sending_mut(local) {
            sending.state = SendState::Landed(landed, machine);
        }
    }

    /// Words that did not go; why is the kernel's, not the interface's to
    /// show (`AGENTS.md`, 文案).
    pub fn failed(&mut self, local: u64) {
        self.changed();
        if let Some(sending) = self.sending_mut(local) {
            sending.state = SendState::Failed;
        }
    }

    fn sending_mut(&mut self, local: u64) -> Option<&mut Sending> {
        self.items.iter_mut().find_map(|item| match item {
            Item::Heard { sending: Some(sending), .. } if sending.local == local => Some(sending),
            _ => None,
        })
    }

    pub fn note(&mut self, text: String, role: Role) {
        self.changed();
        self.items.push(Item::Note { text, role });
    }

    /// Whether anything shown is happening now.
    pub fn moving(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(item, Item::Tool { ended: None, .. } | Item::Prose { writing: true, .. })
                || matches!(item, Item::Heard { sending: Some(Sending { state: SendState::Going, .. }), .. })
        })
    }

    /// The conversation laid out in `width` cells. The words that went in
    /// sit behind `❯` on a band; what the agent writes sits on the ground
    /// unmarked; its thinking is quieter, whole while it thinks and its
    /// first line once it has; the tools it uses in a row form one group, a
    /// line each, with a bar on the left whose colour says how each went. A
    /// blank line comes before each block. A turn that completed says
    /// nothing more; one that did not says how it ended.
    pub fn lines(&self, ctx: &Ctx, width: usize) -> Arc<Vec<Line<'static>>> {
        // Words still being written and tools still running breathe and
        // count their seconds: they are laid out at every frame.
        let moving = self.items.iter().any(|item| {
            matches!(item, Item::Prose { writing: true, .. }) || matches!(item, Item::Tool { ended: None, .. })
        });
        if !moving
            && let Some((at, lines)) = &*self.drawn.borrow()
            && *at == width
        {
            return lines.clone();
        }
        let lines = Arc::new(self.lay_out(ctx, width));
        *self.drawn.borrow_mut() = if moving { None } else { Some((width, lines.clone())) };
        lines
    }

    fn lay_out(&self, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
        let look = &ctx.look;
        let mut out: Vec<Line<'static>> = Vec::new();
        let name_width = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Tool { tool, .. } => Some(text::width(&tool.name)),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let mut last_was_tool = false;
        for item in &self.items {
            let is_tool = matches!(item, Item::Tool { .. });
            match item {
                Item::Heard { from, text, sending, .. } => {
                    blank(&mut out);
                    if let Some(label) = self.speaker(ctx, from.as_ref(), sending.is_some()) {
                        out.push(Line::from(Span::styled(label, look.text(Role::Muted))));
                    }
                    out.extend(said(ctx, text, width));
                    if let Some(sending) = sending {
                        out.extend(sent(ctx, &sending.state, width));
                    }
                }
                Item::Prose { kind: Prose::Thinking, text, writing, laid, took, .. } => {
                    blank(&mut out);
                    out.extend(thinking(ctx, text, *writing, *took, laid, width));
                }
                Item::Prose { kind, text, writing, laid, .. } => {
                    blank(&mut out);
                    if *kind == Prose::Plan {
                        out.push(Line::from(Span::styled(ctx.words.plan().to_string(), look.text(Role::Muted))));
                    }
                    let mut lines = laid.lines(ctx, text, width);
                    if *writing {
                        cursor(ctx, &mut lines, width);
                    }
                    out.extend(lines);
                }
                Item::Tool { tool, at, ended, .. } => {
                    if !last_was_tool {
                        blank(&mut out);
                    }
                    out.push(tool_row(ctx, tool, name_width, *at, *ended, self.here.as_ref(), width));
                    if self.details {
                        out.extend(tool_details(ctx, tool, width));
                    }
                }
                Item::Steps { steps, note, .. } => {
                    blank(&mut out);
                    out.extend(steps_lines(ctx, steps, note.as_deref(), width));
                }
                Item::Compaction { .. } => {
                    blank(&mut out);
                    out.push(Line::from(Span::styled(ctx.words.compacted().to_string(), look.text(Role::Faint))));
                }
                Item::Other { kind, .. } => {
                    blank(&mut out);
                    let line = Line::from(Span::styled(ctx.words.block(kind), look.text(Role::Faint)));
                    out.push(paint::fit_line(&line, width));
                }
                Item::Activity { text, .. } => {
                    blank(&mut out);
                    let line = Line::from(Span::styled(text::first_line(text), look.text(Role::Faint)));
                    out.push(paint::fit_line(&line, width));
                }
                Item::Ended { end, .. } => {
                    if let Some(line) = ended(ctx, end, width) {
                        blank(&mut out);
                        out.push(line);
                    }
                }
                Item::Health { health, .. } => {
                    // Its kind in plain words: what it said is often the
                    // kernel's, not the interface's to show (`AGENTS.md`, 文案).
                    let line = Line::from(vec![Span::styled(
                        format!("▲ {}", ctx.words.health(health.kind)),
                        look.text(Role::Caution),
                    )]);
                    out.push(paint::fit_line(&line, width));
                }
                Item::Note { text, role } => {
                    blank(&mut out);
                    out.extend(text::wrap_plain(text, look.text(*role), width));
                }
                Item::Uncertain => {
                    blank(&mut out);
                    out.extend(text::wrap_plain(ctx.words.landing_unknown(), look.text(Role::Caution), width));
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
            Some(Speaker::Page { page }) => Some(page.to_string()),
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

/// The breathing mark after text still being written.
fn cursor(ctx: &Ctx, lines: &mut Vec<Line<'static>>, width: usize) {
    let breath = ctx.look.faded(Role::Accent, ctx.breath);
    match lines.last_mut() {
        Some(line) if text::line_width(line) + 2 <= width => line.spans.push(Span::styled(" ▍", breath)),
        _ => lines.push(Line::from(Span::styled("▍", breath))),
    }
}

/// What the agent shows of its thinking: `Thinking…`, whole and faint while
/// it thinks; once it has, `Thought for 4s` where the host said how long
/// (`Thought` where not), and its first line, as the official apps fold it. Thinking whose words
/// the host kept (Claude's mostly are) is the line alone.
fn thinking(ctx: &Ctx, text: &str, writing: bool, took: Option<u64>, laid: &Laid, width: usize) -> Vec<Line<'static>> {
    let look = &ctx.look;
    let said = match took {
        _ if writing => ctx.words.thinking().to_string(),
        Some(ms) => ctx.words.thought_for(ms),
        None => ctx.words.thought().to_string(),
    };
    let label = Span::styled(format!("{said}  "), look.text(Role::Faint));
    if text.trim().is_empty() {
        let mut lines = vec![Line::from(label)];
        if writing {
            cursor(ctx, &mut lines, width);
        }
        return lines.iter().map(|line| paint::fit_line(line, width)).collect();
    }
    if !writing {
        let first = text.lines().map(|line| line.trim().trim_matches('*').trim()).find(|line| !line.is_empty());
        let line = Line::from(vec![label, Span::styled(first.unwrap_or_default().to_string(), look.text(Role::Faint))]);
        return vec![paint::fit_line(&line, width)];
    }
    let mut lines = laid.lines(ctx, text, width);
    for line in &mut lines {
        for span in &mut line.spans {
            span.style = span.style.patch(look.text(Role::Faint));
        }
    }
    cursor(ctx, &mut lines, width);
    let mut out = vec![Line::from(label)];
    out.extend(lines);
    out
}

/// The agent's steps, one a line: done ones quiet, the one being done
/// marked, the rest waiting.
fn steps_lines(ctx: &Ctx, steps: &[Step], note: Option<&str>, width: usize) -> Vec<Line<'static>> {
    let look = &ctx.look;
    let mut out = vec![Line::from(Span::styled(ctx.words.steps().to_string(), look.text(Role::Muted)))];
    if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
        out.extend(text::wrap_plain(note, look.text(Role::Faint), width));
    }
    for step in steps {
        let (mark, mark_role, role) = match step.status {
            StepStatus::Done => ("✓", Role::Faint, Role::Faint),
            StepStatus::Doing => ("▸", Role::Accent, Role::Text),
            StepStatus::Pending => ("·", Role::Muted, Role::Muted),
        };
        let line = Line::from(vec![
            Span::styled(format!("{mark} "), look.text(mark_role)),
            Span::styled(text::first_line(&step.text), look.text(role)),
        ]);
        out.push(paint::fit_line(&line, width));
    }
    out
}

/// The bar before a tool's row.
const BAR: &str = "▎";
/// The words that went in start this far in, after `❯`.
const MARGIN: usize = 2;

/// `lines` behind `mark`: the mark and a space before the first, as much
/// room before the rest.
fn behind(mark: Span<'static>, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            let lead = if index == 0 { vec![mark.clone(), Span::raw(" ")] } else { vec![Span::raw("  ")] };
            line.spans.splice(0..0, lead);
            line
        })
        .collect()
}

/// Words that went in, behind a quieter `❯` on a band across the column;
/// without a colour for the band, behind `❯` alone.
fn said(ctx: &Ctx, words: &str, width: usize) -> Vec<Line<'static>> {
    let look = &ctx.look;
    let banded = look.color(Role::Bubble).is_some();
    let style = if banded { look.bubble() } else { look.text(Role::Text) };
    let mark = match look.color(Role::Muted) {
        Some(muted) => style.fg(muted),
        None => style,
    };
    let lines = text::wrap_plain(words, style, width.saturating_sub(MARGIN));
    let mut out = behind(Span::styled("❯", mark), lines);
    if banded {
        for line in &mut out {
            let used = text::line_width(line);
            line.spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), style));
            for span in &mut line.spans {
                if span.style.bg.is_none() {
                    span.style.bg = style.bg;
                }
            }
        }
    }
    out
}

/// What the World said of words sent from here.
fn sent(ctx: &Ctx, state: &SendState, width: usize) -> Vec<Line<'static>> {
    let words = &ctx.words;
    let (text, role) = match state {
        SendState::Going => (words.sending().to_string(), Role::Faint),
        SendState::Failed => (words.not_sent().to_string(), Role::Wrong),
        SendState::Landed(landed, machine) => match landed {
            Landed::NewTurn { .. } => return Vec::new(),
            Landed::JoinedTurn { .. } => (words.joined_turn().to_string(), Role::Faint),
            Landed::Handed { .. } => (words.handed().to_string(), Role::Faint),
            Landed::Uncertain { .. } => (words.landing_unknown().to_string(), Role::Caution),
            Landed::Sent => (words.sent_to(machine.as_str()), Role::Faint),
            Landed::Waiting { why, answered } => {
                let mut text = match why {
                    Some(_) => words.waiting_to_deliver().to_string(),
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

/// A tool in one row: a bar whose colour says how it went (the colour of
/// a rule once it is done, the accent breathing while it runs, the colour
/// of what went wrong when it failed), its name, what it was used on, the
/// machine it acted on when not this one, and on the right how long it took
/// or how its command ended. A tool read back from a host's record that
/// keeps no times began and ended at the same instant: how long it took is
/// not known, and is not shown.
fn tool_row(
    ctx: &Ctx,
    tool: &Tool,
    name_width: usize,
    at: Timestamp,
    ended: Option<(Timestamp, Use)>,
    here: Option<&MachineName>,
    width: usize,
) -> Line<'static> {
    let look = &ctx.look;
    let words = &ctx.words;
    let seconds = match ended {
        Some((end, _)) => end.as_second() - at.as_second(),
        None => ctx.now.as_second() - at.as_second(),
    };
    let took = match (tool.duration_ms, ended) {
        (Some(ms), Some(_)) if ms < 1000 => Some(words.under_a_second().to_string()),
        (Some(ms), Some(_)) => Some(words.duration(ms / 1000)),
        (None, Some((end, _))) if end == at => None,
        (None, Some(_)) if seconds < 1 => Some(words.under_a_second().to_string()),
        _ => Some(words.duration(seconds.max(0) as u64)),
    };
    let (exit, job) = match &tool.does {
        Does::Command { exit, job, .. } => (*exit, job.clone()),
        _ => (None, None),
    };
    let (bar, detail_role, said) = match ended {
        None => (
            look.faded(Role::Accent, ctx.breath),
            Role::Text,
            Span::styled(took.unwrap_or_default(), look.text(Role::Faint)),
        ),
        Some((_, Use::Done)) => match (exit, job) {
            (_, Some(job)) => {
                (look.text(Role::Rule), Role::Muted, Span::styled(words.went_on_as(&job), look.text(Role::Accent)))
            }
            (Some(code), None) if code != 0 => (
                look.text(Role::Caution),
                Role::Muted,
                Span::styled(words.exited(code, took.as_deref()), look.text(Role::Caution)),
            ),
            _ => (look.text(Role::Rule), Role::Muted, Span::styled(took.unwrap_or_default(), look.text(Role::Faint))),
        },
        Some((_, Use::Failed)) => (
            look.text(Role::Wrong),
            Role::Muted,
            Span::styled(words.tool_failed(took.as_deref()), look.text(Role::Wrong)),
        ),
        Some((_, Use::Declined)) => (
            look.text(Role::Wrong),
            Role::Muted,
            Span::styled(words.tool_declined().to_string(), look.text(Role::Wrong)),
        ),
        Some((_, Use::Unknown)) => (look.text(Role::Rule), Role::Muted, Span::styled("?", look.text(Role::Faint))),
        Some((_, Use::Cut)) => {
            (look.text(Role::Rule), Role::Faint, Span::styled(words.tool_cut().to_string(), look.text(Role::Faint)))
        }
    };
    let pad = name_width.saturating_sub(text::width(&tool.name));
    let mut spans = vec![
        Span::styled(BAR, bar),
        Span::raw(" "),
        Span::styled(format!("{}{}", tool.name, " ".repeat(pad)), look.text(Role::Faint)),
        Span::raw("  "),
        Span::styled(fastctx_client::shown::tool_detail(tool), look.text(detail_role)),
    ];
    if let Some(machine) = tool.machine.as_ref().filter(|machine| Some(*machine) != here) {
        spans.push(Span::styled(format!("  · {machine}"), look.text(Role::Muted)));
    }
    let left = Line::from(spans);
    let right_width = text::width(&said.content);
    let mut line = paint::fit_line(&left, width.saturating_sub(right_width + 2));
    let used = text::line_width(&line);
    line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used + right_width))));
    line.spans.push(said);
    line
}

/// Lines of a tool's output shown under it at most, the last ones; of a
/// change's diff, the first ones. What is left out is said.
const OUTPUT_LINES: usize = 20;
const DIFF_LINES: usize = 40;

/// What a tool was given and gave back, under its row and its bar: the
/// command and what it printed, the diff of each file it changed, its
/// arguments and its answer.
fn tool_details(ctx: &Ctx, tool: &Tool, width: usize) -> Vec<Line<'static>> {
    let look = &ctx.look;
    let words = &ctx.words;
    let inner = width.saturating_sub(4);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let plain = |lines: &mut Vec<Line<'static>>, text: &str, role: Role| {
        lines.extend(text::wrap_plain(text, look.text(role), inner));
    };
    match &tool.does {
        Does::Command { command, cwd, .. } => {
            for (index, line) in command.lines().enumerate() {
                let lead = if index == 0 { "$ " } else { "  " };
                plain(&mut lines, &format!("{lead}{line}"), Role::Text);
            }
            if let Some(cwd) = cwd {
                plain(&mut lines, &words.in_directory(cwd), Role::Faint);
            }
        }
        Does::Edit { changes } => {
            for change in changes {
                let path = match &change.how {
                    fastctx_types::session::ChangeHow::Moved { to } => format!("{} → {to}", change.path),
                    _ => change.path.clone(),
                };
                plain(&mut lines, &path, Role::Muted);
                let diff: Vec<&str> = change.diff.lines().collect();
                for line in diff.iter().take(DIFF_LINES) {
                    let role = match line.chars().next() {
                        Some('+') => Role::Alive,
                        Some('-') => Role::Wrong,
                        _ => Role::Faint,
                    };
                    plain(&mut lines, line, role);
                }
                if diff.len() > DIFF_LINES {
                    plain(&mut lines, &words.more_lines_after(diff.len() - DIFF_LINES), Role::Faint);
                }
            }
        }
        _ => {
            if let Some(input) = tool.input.as_object() {
                for (key, value) in input {
                    let value = match value {
                        serde_json::Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    let mut value_lines = value.lines();
                    let first = value_lines.next().unwrap_or_default();
                    let more = value_lines.count();
                    let said = if more > 0 {
                        format!("{key}: {first} {}", words.more_lines_after(more))
                    } else {
                        format!("{key}: {first}")
                    };
                    plain(&mut lines, &said, Role::Muted);
                }
            }
        }
    }
    // What a World run's `edit`, `write` and `apply_patch` changed, from the
    // rows kept with the conversation; a larger change says only how much.
    if let Some(run) = &tool.run {
        use fastctx_types::run_facts::DiffLineKind;
        for write in &run.writes {
            let counted = match (write.added, write.removed) {
                (Some(added), Some(removed)) => format!(" +{added} -{removed}"),
                _ => String::new(),
            };
            plain(&mut lines, &format!("{}{counted}", write.path), Role::Muted);
            let Some(rows) = write.diff.as_ref().and_then(|diff| diff.lines.as_ref()) else { continue };
            // A first hunk at the very top of the file stands for nothing.
            let top = rows.first().is_some_and(|row| row.kind == DiffLineKind::Hunk && row.before == Some(1));
            let rows = &rows[usize::from(top)..];
            for row in rows.iter().take(DIFF_LINES) {
                let (text, role) = match row.kind {
                    DiffLineKind::Added => (format!("+{}", row.text), Role::Alive),
                    DiffLineKind::Removed => (format!("-{}", row.text), Role::Wrong),
                    DiffLineKind::Context => (format!(" {}", row.text), Role::Faint),
                    DiffLineKind::Hunk => ("⋯".to_string(), Role::Faint),
                };
                plain(&mut lines, &text, role);
            }
            if rows.len() > DIFF_LINES {
                plain(&mut lines, &words.more_lines_after(rows.len() - DIFF_LINES), Role::Faint);
            }
        }
    }
    let mut output: Vec<String> = Vec::new();
    // A World run that went slimmed carries the model's copy of its output
    // as facts alone (`fastctx_net::history::slim`).
    if let Some(run) = &tool.run
        && !tool.output.iter().any(|piece| matches!(piece, fastctx_types::session::Content::Text { .. }))
    {
        use fastctx_types::run_facts::DisplayPiece;
        for span in &run.spans {
            match &span.piece {
                DisplayPiece::Text { text, .. } | DisplayPiece::PartOfLine { text, .. } => {
                    output.extend(text.lines().map(str::to_string))
                }
                DisplayPiece::Omitted { first_line, last_line } => {
                    output.push(words.more_lines_after((last_line.saturating_sub(*first_line) + 1) as usize))
                }
                DisplayPiece::Image { .. } => {}
                DisplayPiece::Unreadable { reason, .. } => output.push(reason.clone()),
            }
        }
    }
    for piece in &tool.output {
        match piece {
            fastctx_types::session::Content::Text { text } => output.extend(text.lines().map(str::to_string)),
            fastctx_types::session::Content::Image { mime, .. }
            | fastctx_types::session::Content::ImageRef { mime, .. } => output.push(words.image(mime)),
            fastctx_types::session::Content::ImageUrl { url } | fastctx_types::session::Content::AudioUrl { url } => {
                output.push(url.clone())
            }
            // A block of a kind the World does not read, by its type.
            fastctx_types::session::Content::Other { block } => {
                output.push(words.block(block.get("type").and_then(serde_json::Value::as_str).unwrap_or("?")))
            }
        }
    }
    if !matches!(tool.does, Does::Edit { .. }) || tool.status != fastctx_types::session::ToolStatus::Done {
        if output.len() > OUTPUT_LINES {
            plain(&mut lines, &words.more_lines_before(output.len() - OUTPUT_LINES), Role::Faint);
        }
        for line in output.iter().skip(output.len().saturating_sub(OUTPUT_LINES)) {
            plain(&mut lines, line, Role::Faint);
        }
    }
    let bar = Span::styled(format!("{BAR}   "), look.text(Role::Rule));
    lines
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, bar.clone());
            line
        })
        .collect()
}

/// How a turn ended, when it did not complete.
fn ended(ctx: &Ctx, end: &TurnEnd, width: usize) -> Option<Line<'static>> {
    let words = &ctx.words;
    let reason = end.reason.as_deref().map(text::first_line);
    let (said, role) = match &end.how {
        TurnHow::Completed => return None,
        TurnHow::Failed => (words.turn_failed(reason.as_deref()), Role::Wrong),
        // Stopped by someone, not cut short by anything going wrong.
        TurnHow::Interrupted if reason.is_none() => (words.turn_stopped().to_string(), Role::Muted),
        TurnHow::Interrupted => (words.turn_interrupted(reason.as_deref()), Role::Caution),
        TurnHow::Other(how) => (words.turn_ended_as(how), Role::Muted),
    };
    Some(paint::fit_line(&Line::from(Span::styled(said, ctx.look.text(role))), width))
}
