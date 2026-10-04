//! Reading one journal (`world-journals.md`, 读): what it covers, then its
//! entries in order, each with the World's stamp, what it answers and who
//! mentioned it later. The latest entries come first to hand; earlier ones
//! are a key away.

use crossterm::event::{KeyCode, KeyEvent};
use fastctx_types::project::{AttachmentKind, EntryView, JournalView, Kept};
use fastctx_types::{JournalName, TownName};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::project::writer_name;
use super::{quiet, title, wrong};
use crate::app::{Act, Ctx, Data, Done, Job};
use crate::design::Role;
use crate::paint::{self, Row};
use crate::text;

pub struct JournalPage {
    town: TownName,
    project: String,
    journal: JournalName,
    /// Each part read so far, earliest first.
    parts: Vec<JournalView>,
    failed: Option<String>,
    reading: bool,
    /// The first row shown; `None` follows the end.
    top: Option<usize>,
    last_total: usize,
    last_height: usize,
}

impl JournalPage {
    pub fn new(town: TownName, project: String, journal: JournalName) -> Self {
        Self {
            town,
            project,
            journal,
            parts: Vec::new(),
            failed: None,
            reading: false,
            top: None,
            last_total: 0,
            last_height: 0,
        }
    }

    /// The page as it would look with this part read, for previews.
    pub fn with(project: String, view: JournalView) -> Self {
        let mut page = Self::new(view.town.clone(), project, view.journal.clone());
        page.parts.push(view);
        page
    }

    /// What to ask when the page opens.
    pub fn start(&mut self) -> Option<Job> {
        self.reading = true;
        Some(Job::Journal { town: self.town.clone(), journal: self.journal.clone(), before: None })
    }

    pub fn data(&mut self, data: Data) -> Act {
        if let Data::Done { result, .. } = data {
            self.reading = false;
            match result {
                Ok(Done::Journal(view)) => {
                    self.failed = None;
                    let earlier = !self.parts.is_empty();
                    self.parts.insert(0, *view);
                    // Reading further back starts at what was just read.
                    if earlier {
                        self.top = Some(0);
                    }
                }
                Ok(_) => {}
                Err(why) => self.failed = Some(why),
            }
        }
        Act::Stay
    }

    fn first(&self) -> Option<&EntryView> {
        self.parts.first().and_then(|part| part.entries.first())
    }

    pub fn key(&mut self, _ctx: &Ctx, key: KeyEvent) -> Act {
        let bottom = self.last_total.saturating_sub(self.last_height);
        let top = self.top.unwrap_or(bottom);
        let page = self.last_height.saturating_sub(2).max(1);
        let to = |top: usize| if top >= bottom { None } else { Some(top) };
        match key.code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => return Act::Back,
            KeyCode::Up | KeyCode::Char('k') => self.top = to(top.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.top = to(top + 1),
            KeyCode::PageUp => self.top = to(top.saturating_sub(page)),
            KeyCode::PageDown | KeyCode::Char(' ') => self.top = to(top + page),
            KeyCode::Home => self.top = to(0),
            KeyCode::End => self.top = None,
            KeyCode::Char('[') if !self.reading && self.parts.first().is_some_and(|part| part.first > 1) => {
                if let Some(first) = self.first() {
                    let before = Some(first.stamp.id.clone());
                    self.reading = true;
                    return Act::Do(Job::Journal { town: self.town.clone(), journal: self.journal.clone(), before });
                }
            }
            _ => {}
        }
        Act::Stay
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect) {
        let words = &ctx.words;
        let look = &ctx.look;
        let parts = paint::frame(area);
        let width = parts.body.width as usize;
        let mut rows: Vec<Row> = Vec::new();

        let latest = self.parts.last();
        if let Some(description) = latest.and_then(|part| part.description.as_ref()) {
            for line in text::wrap_plain(&description.text, look.text(Role::Muted), width) {
                rows.push(Row::new(line));
            }
            rows.push(Row::new(quiet(
                ctx,
                words.described(&writer_name(ctx, &description.by), &crate::words::clock(description.at, ctx.now)),
            )));
            rows.push(Row::blank());
        }
        if let Some(part) = self.parts.first()
            && part.first > 1
        {
            rows.push(Row::new(quiet(ctx, words.earlier_entries(part.first - 1))));
            rows.push(Row::blank());
        }
        for entry in self.parts.iter().flat_map(|part| &part.entries) {
            rows.extend(entry_rows(ctx, entry, width));
            rows.push(Row::blank());
        }

        let buffer = frame.buffer_mut();
        let right = match (self.parts.first(), self.parts.last()) {
            (Some(first), Some(last)) => {
                quiet(ctx, words.entries_shown(first.first, last.first + last.entries.len() - 1, last.total))
            }
            _ => Line::default(),
        };
        let crumbs = vec![words.world().to_string(), self.project.clone(), self.journal.to_string()];
        paint::header(buffer, parts.header, look, &crumbs, &right);
        let height = parts.body.height as usize;
        self.last_total = rows.len();
        self.last_height = height;
        let bottom = rows.len().saturating_sub(height);
        let top = self.top.map_or(bottom, |top| top.min(bottom));
        paint::rows(buffer, parts.body, &rows, top, None, look);

        let note = match (&self.failed, self.reading) {
            (Some(why), _) => Some(wrong(ctx, text::first_line(why))),
            (None, true) => Some(quiet(ctx, words.reading())),
            _ => None,
        };
        if let Some(note) = note {
            paint::line(buffer, parts.note.x, parts.note.y, parts.note.width, &note, &Line::default(), false, look);
        }
        let mut keys = vec![("↑↓ PgUp PgDn", words.key_scroll())];
        if self.parts.first().is_some_and(|part| part.first > 1) {
            keys.push(("[", words.key_earlier()));
        }
        keys.push(("Esc", words.key_back()));
        paint::footer(buffer, parts.footer, look, &keys);
    }
}

/// One entry: the World's stamp, its text, its attachments, and how it is
/// tied to other entries.
fn entry_rows(ctx: &Ctx, entry: &EntryView, width: usize) -> Vec<Row> {
    let words = &ctx.words;
    let look = &ctx.look;
    let stamp = &entry.stamp;
    let mut who = vec![writer_name(ctx, &stamp.by)];
    if let Some(agent) = &stamp.agent {
        who.push(format!("{} {}", words.agent(agent.agent.as_str()), agent.version).trim().to_string());
    }
    who.push(stamp.machine.to_string());
    if let Some(model) = &stamp.model {
        who.push(if model.self_reported { words.self_reported(&model.name) } else { model.name.clone() });
    }
    let mut rows = vec![
        Row::new(Line::from(vec![
            title(ctx, crate::words::clock(stamp.at, ctx.now)),
            Span::styled(format!("   {}", who.join(" · ")), look.text(Role::Muted)),
        ]))
        .right(quiet(ctx, stamp.id.to_string())),
    ];
    if !stamp.cites.is_empty() {
        let cites: Vec<String> = stamp.cites.iter().map(|id| id.to_string()).collect();
        rows.push(Row::new(Span::styled(words.answers(&cites.join(", ")), look.text(Role::Link))));
    }
    for line in crate::markdown::render(&entry.text, width, look) {
        rows.push(Row::new(line));
    }
    if !stamp.attachments.is_empty() {
        let listed: Vec<String> = stamp
            .attachments
            .iter()
            .map(|attachment| {
                let kind = match attachment.kind {
                    AttachmentKind::Text => words.kind_text(),
                    AttachmentKind::Image => words.kind_image(),
                    AttachmentKind::Pdf => "PDF",
                    AttachmentKind::Binary => words.kind_binary(),
                };
                let size = bytes(attachment.bytes);
                let kept_on = match &attachment.kept {
                    Kept::InEntry => None,
                    Kept::OnMachine { location } => Some(location.machine.as_str()),
                };
                words.attachment(&attachment.name, kind, &size, kept_on)
            })
            .collect();
        for line in text::wrap_plain(&words.attachments(&listed.join(" · ")), look.text(Role::Faint), width) {
            rows.push(Row::new(line));
        }
    }
    if !entry.mentioned_by.is_empty() {
        let later: Vec<String> = entry.mentioned_by.iter().map(|id| id.to_string()).collect();
        rows.push(Row::new(Span::styled(words.mentioned_by(&later.join(", ")), look.text(Role::Link))));
    }
    rows
}

/// A size as people read it.
fn bytes(bytes: u64) -> String {
    match bytes {
        0..1_024 => format!("{bytes} B"),
        1_024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1_024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
        _ => format!("{:.1} GB", bytes as f64 / 1_073_741_824.0),
    }
}
