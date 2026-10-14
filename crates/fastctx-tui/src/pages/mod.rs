//! The pages of the TUI and what they share.

pub mod form;
pub mod journal;
mod logins;
pub mod machines;
mod program;
pub mod project;
pub mod talk;
pub mod world;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::app::{Act, Ctx, Data, Feed, Job};
use crate::design::Role;
use crate::paint::{self, Row};
use crate::text;

pub enum Page {
    World(Box<world::WorldPage>),
    Project(Box<project::ProjectPage>),
    Talk(Box<talk::TalkPage>),
    Journal(Box<journal::JournalPage>),
    Machines(Box<machines::MachinesPage>),
    Form(Box<form::FormPage>),
}

impl Page {
    /// Whether the page belongs to talking with agents, which shows only
    /// while conversations are on (`world-gui.md`, 功能).
    pub fn of_conversations(&self) -> bool {
        matches!(self, Page::Project(_) | Page::Talk(_) | Page::Journal(_))
    }

    /// A turn's end to tell the user of, from a conversation.
    pub fn take_notice(&mut self) -> Option<(String, String)> {
        match self {
            Page::Talk(page) => page.take_notice(),
            _ => None,
        }
    }

    pub fn key(&mut self, ctx: &Ctx, key: KeyEvent) -> Act {
        match self {
            Page::World(page) => page.key(ctx, key),
            Page::Project(page) => page.key(ctx, key),
            Page::Talk(page) => page.key(ctx, key),
            Page::Journal(page) => page.key(ctx, key),
            Page::Machines(page) => page.key(ctx, key),
            Page::Form(page) => page.key(ctx, key),
        }
    }

    /// What the page asks of the World as it opens.
    pub fn start(&mut self) -> Option<Job> {
        match self {
            Page::Journal(page) => page.start(),
            _ => None,
        }
    }

    pub fn paste(&mut self, text: String) -> Act {
        match self {
            Page::Talk(page) => page.paste(text),
            Page::Form(page) => page.paste(text),
            _ => Act::Stay,
        }
    }

    pub fn data(&mut self, ctx: &Ctx, data: Data) -> Act {
        match self {
            Page::World(page) => page.data(data),
            Page::Project(page) => page.data(data),
            Page::Talk(page) => page.data(ctx, data),
            Page::Journal(page) => page.data(data),
            Page::Machines(page) => page.data(data),
            Page::Form(page) => page.data(ctx, data),
        }
    }

    pub fn feed(&self) -> Option<Feed> {
        match self {
            Page::World(page) => Some(page.feed()),
            Page::Project(page) => Some(page.feed()),
            Page::Talk(page) => page.feed(),
            Page::Journal(_) => None,
            Page::Machines(_) => Some(Feed::Nodes),
            Page::Form(page) => page.feed(),
        }
    }

    pub fn animating(&self) -> bool {
        match self {
            Page::World(page) => page.animating(),
            Page::Project(page) => page.animating(),
            Page::Talk(page) => page.animating(),
            Page::Journal(_) | Page::Machines(_) | Page::Form(_) => false,
        }
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect, pasting: Option<usize>) {
        paint::ground(frame.buffer_mut(), area, &ctx.look);
        match self {
            Page::World(page) => page.draw(ctx, frame, area),
            Page::Project(page) => page.draw(ctx, frame, area),
            Page::Talk(page) => page.draw(ctx, frame, area, pasting),
            Page::Journal(page) => page.draw(ctx, frame, area),
            Page::Machines(page) => page.draw(ctx, frame, area),
            Page::Form(page) => page.draw(ctx, frame, area, pasting),
        }
    }
}

/// A page of selectable items laid out as rows: which item is selected and
/// where the view starts.
#[derive(Default)]
pub struct List {
    pub selected: usize,
    top: usize,
}

impl List {
    /// Moves the selection for a key among `count` items; returns whether
    /// the key was one that moves it.
    pub fn key(&mut self, key: &KeyEvent, count: usize) -> bool {
        let last = count.saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(5),
            KeyCode::PageDown => self.selected = (self.selected + 5).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            _ => return false,
        }
        true
    }

    /// Keeps the selection within `count` items as they come and go.
    pub fn clamp(&mut self, count: usize) {
        self.selected = self.selected.min(count.saturating_sub(1));
    }
}

/// What a list page shows.
pub struct Sheet<'a> {
    pub crumbs: Vec<String>,
    pub right: Line<'static>,
    pub rows: Vec<Row>,
    pub keys: Vec<(&'a str, &'a str)>,
    pub note: Option<Line<'static>>,
}

/// Paints a list page, keeping the selected item in view.
pub fn sheet(ctx: &Ctx, frame: &mut Frame, area: Rect, sheet: &Sheet<'_>, list: &mut List) {
    let parts = paint::frame(area);
    let buffer = frame.buffer_mut();
    paint::header(buffer, parts.header, &ctx.look, &sheet.crumbs, &sheet.right);
    let height = parts.body.height as usize;
    let any_item = sheet.rows.iter().any(|row| row.item.is_some());
    if any_item && let Some((first, last)) = paint::item_rows(&sheet.rows, list.selected) {
        // The section title just above an item comes into view with it.
        let first = if first > 0 && sheet.rows[first - 1].item.is_none() { first - 1 } else { first };
        list.top = paint::keep_in_view(list.top, first, last, height);
    }
    list.top = list.top.min(sheet.rows.len().saturating_sub(height));
    let selected = any_item.then_some(list.selected);
    paint::rows(buffer, parts.body, &sheet.rows, list.top, selected, &ctx.look);
    if list.top > 0 {
        text::Layout::default().write_line(
            buffer,
            Rect::new(parts.body.right().saturating_sub(1), parts.body.y, 1, 1),
            &Line::styled("↑", ctx.look.text(Role::Faint)),
        );
    }
    if list.top + height < sheet.rows.len() {
        text::Layout::default().write_line(
            buffer,
            Rect::new(parts.body.right().saturating_sub(1), parts.body.bottom().saturating_sub(1), 1, 1),
            &Line::styled("↓", ctx.look.text(Role::Faint)),
        );
    }
    if let Some(note) = &sheet.note {
        paint::line(buffer, parts.note.x, parts.note.y, parts.note.width, note, &Line::default(), false, &ctx.look);
    }
    paint::footer(buffer, parts.footer, &ctx.look, &sheet.keys);
}

/// A note that something went wrong.
pub fn wrong(ctx: &Ctx, text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), ctx.look.text(Role::Wrong)))
}

/// A quiet note.
pub fn quiet(ctx: &Ctx, text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), ctx.look.text(Role::Faint)))
}

/// The name of a thing, as the eye should find it first.
pub fn title(ctx: &Ctx, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), ctx.look.text(Role::Text).add_modifier(Modifier::BOLD))
}
