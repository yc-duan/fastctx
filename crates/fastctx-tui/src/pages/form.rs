//! Asking the World for something that takes a few answers: a conversation
//! with a new agent, a project opened or its record written anew, a machine
//! invited, joined, revoked or left. Choices come from what the World says
//! now: the machines online, the projects that span the chosen machine.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_types::project::{Listing, Location, Record, RecordFields, TownSummary};
use fastctx_types::world::NodesReply;
use fastctx_types::{AgentName, MachineName, TownName};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use super::{Page, project, quiet, talk, wrong};
use crate::app::{Act, Ctx, Data, Done, Feed, Job};
use crate::design::Role;
use crate::editor::{Edit, Editor};
use crate::paint::{self, Row};
use crate::text;
use crate::words::FieldName as Key;

pub struct FormPage {
    kind: Kind,
    fields: Vec<Field>,
    focus: usize,
    note: Option<Line<'static>>,
    working: bool,
    /// What the World answered, when it is something to read.
    result: Option<Vec<Line<'static>>>,
    nodes: Option<NodesReply>,
    towns: Vec<TownSummary>,
    top: usize,
}

enum Kind {
    OpenAgent { preset: Option<TownName> },
    NewProject,
    EditProject { town: TownName, machines: Vec<MachineName> },
    Invite,
    Join,
    Revoke { name: MachineName },
    Leave,
}

struct Field {
    key: Key,
    value: Value,
}

enum Value {
    Text(Editor),
    /// One of several: what is shown, and the value it stands for.
    Choice {
        options: Vec<(String, String)>,
        index: usize,
    },
    /// Any of several, ticked or not; `at` is the one the keys are on.
    Ticks {
        options: Vec<(String, bool)>,
        at: usize,
    },
}

impl Field {
    fn text(key: Key, editor: Editor) -> Self {
        Self { key, value: Value::Text(editor) }
    }

    fn choice(key: Key, options: Vec<(String, String)>) -> Self {
        Self { key, value: Value::Choice { options, index: 0 } }
    }

    fn ticks(key: Key) -> Self {
        Self { key, value: Value::Ticks { options: Vec::new(), at: 0 } }
    }

    fn text_value(&self) -> String {
        match &self.value {
            Value::Text(editor) => editor.text().trim().to_string(),
            Value::Choice { options, index } => options.get(*index).map(|(_, value)| value.clone()).unwrap_or_default(),
            Value::Ticks { .. } => String::new(),
        }
    }
}

impl FormPage {
    fn new(kind: Kind, fields: Vec<Field>) -> Self {
        Self {
            kind,
            fields,
            focus: 0,
            note: None,
            working: false,
            result: None,
            nodes: None,
            towns: Vec::new(),
            top: 0,
        }
    }

    pub fn open_agent(preset: Option<TownName>) -> Self {
        let agents = vec![("Codex".to_string(), "codex".to_string()), ("Claude".to_string(), "claude".to_string())];
        Self::new(
            Kind::OpenAgent { preset },
            vec![
                Field::choice(Key::Agent, agents),
                Field::choice(Key::Machine, Vec::new()),
                Field::choice(Key::Project, Vec::new()),
                Field::text(Key::FirstWords, Editor::multi()),
            ],
        )
    }

    pub fn new_project() -> Self {
        Self::new(
            Kind::NewProject,
            vec![
                Field::text(Key::Slug, Editor::single()),
                Field::text(Key::Name, Editor::single()),
                Field::text(Key::What, Editor::multi()),
                Field::ticks(Key::Machines),
                Field::text(Key::Places, Editor::multi()),
                Field::text(Key::Canon, Editor::single()),
            ],
        )
    }

    pub fn edit_project(town: TownName, record: &Record) -> Self {
        let places: Vec<String> = record
            .places
            .iter()
            .map(|place| format!("{} | {} | {}", place.what, place.location.machine, place.location.path))
            .collect();
        let canon =
            record.canon.as_ref().map(|canon| format!("{} | {}", canon.machine, canon.path)).unwrap_or_default();
        Self::new(
            Kind::EditProject { town, machines: record.machines.clone() },
            vec![
                Field::text(Key::Name, Editor::single().with(&record.name)),
                Field::text(Key::What, Editor::multi().with(&record.what)),
                Field::ticks(Key::Machines),
                Field::text(Key::Places, Editor::multi().with(&places.join("\n"))),
                Field::text(Key::Canon, Editor::single().with(&canon)),
            ],
        )
    }

    pub fn invite() -> Self {
        Self::new(
            Kind::Invite,
            vec![Field::text(Key::InviteName, Editor::single()), Field::text(Key::Hours, Editor::single().with("24"))],
        )
    }

    pub fn join() -> Self {
        Self::new(
            Kind::Join,
            vec![Field::text(Key::Code, Editor::single()), Field::text(Key::JoinName, Editor::single())],
        )
    }

    pub fn revoke(name: MachineName) -> Self {
        Self::new(Kind::Revoke { name }, vec![Field::text(Key::Reason, Editor::single())])
    }

    pub fn leave() -> Self {
        Self::new(Kind::Leave, Vec::new())
    }

    pub fn feed(&self) -> Option<Feed> {
        match self.kind {
            Kind::OpenAgent { .. } | Kind::NewProject | Kind::EditProject { .. } if self.result.is_none() => {
                Some(Feed::Choices)
            }
            _ => None,
        }
    }

    fn field(&mut self, key: Key) -> Option<&mut Field> {
        self.fields.iter_mut().find(|field| field.key == key)
    }

    fn value(&self, key: Key) -> String {
        self.fields.iter().find(|field| field.key == key).map(Field::text_value).unwrap_or_default()
    }

    pub fn paste(&mut self, text: String) -> Act {
        if let Some(Field { value: Value::Text(editor), .. }) = self.fields.get_mut(self.focus) {
            editor.insert(&text);
        }
        Act::Stay
    }

    pub fn data(&mut self, ctx: &Ctx, data: Data) -> Act {
        match data {
            Data::Choices { nodes, towns } => {
                if let Ok(nodes) = nodes {
                    self.nodes = Some(nodes);
                }
                if let Ok(towns) = towns {
                    self.towns = towns;
                }
                self.refresh_choices(ctx);
                Act::Stay
            }
            Data::Done { result, .. } => {
                self.working = false;
                match result {
                    Ok(done) => self.done(ctx, done),
                    Err(why) => {
                        self.note = Some(wrong(ctx, text::first_line(&why)));
                        Act::Stay
                    }
                }
            }
            _ => Act::Stay,
        }
    }

    /// The choices as the World stands now, keeping what was chosen.
    fn refresh_choices(&mut self, ctx: &Ctx) {
        let Some(nodes) = &self.nodes else { return };
        let here = nodes.machine.clone();
        let mut usable: Vec<MachineName> = nodes
            .machines
            .iter()
            .filter(|node| node.revoked.is_none() && node.online != Some(false))
            .map(|node| node.name.clone())
            .collect();
        if !usable.contains(&here) {
            usable.insert(0, here.clone());
        }
        let every: Vec<MachineName> =
            nodes.machines.iter().filter(|node| node.revoked.is_none()).map(|node| node.name.clone()).collect();
        let every = if every.is_empty() { vec![here.clone()] } else { every };
        let towns = self.towns.clone();
        let words = ctx.words;
        let initial = match &self.kind {
            Kind::EditProject { machines, .. } => machines.clone(),
            _ => vec![here.clone()],
        };
        let preset = match &self.kind {
            Kind::OpenAgent { preset } => preset.clone(),
            _ => None,
        };

        if let Some(Field { value: Value::Choice { options, index }, .. }) = self.field(Key::Machine) {
            let chosen = options.get(*index).map(|(_, value)| value.clone()).unwrap_or_else(|| here.to_string());
            *options = usable
                .iter()
                .map(|machine| {
                    let shown = if *machine == here {
                        format!("{machine}（{}）", words.this_machine())
                    } else {
                        machine.to_string()
                    };
                    (shown, machine.to_string())
                })
                .collect();
            *index = options.iter().position(|(_, value)| *value == chosen).unwrap_or(0);
        }
        let machine = self.value(Key::Machine);
        if let Some(Field { value: Value::Choice { options, index }, .. }) = self.field(Key::Project) {
            let chosen = options.get(*index).map(|(_, value)| value.clone()).or(preset.map(|town| town.to_string()));
            let mut list = vec![(words.no_project().to_string(), String::new())];
            list.extend(
                towns
                    .iter()
                    .filter(|town| town.record.machines.iter().any(|spans| spans.as_str() == machine))
                    .map(|town| (town.record.name.clone(), town.town.to_string())),
            );
            *options = list;
            *index = chosen.and_then(|chosen| options.iter().position(|(_, value)| *value == chosen)).unwrap_or(0);
        }
        if let Some(Field { value: Value::Ticks { options, .. }, .. }) = self.field(Key::Machines) {
            let first = options.is_empty();
            let ticked: Vec<String> = options.iter().filter(|(_, on)| *on).map(|(name, _)| name.clone()).collect();
            *options = every
                .iter()
                .map(|machine| {
                    let on = if first { initial.contains(machine) } else { ticked.contains(&machine.to_string()) };
                    (machine.to_string(), on)
                })
                .collect();
            // A machine the record names that is not in the World any more
            // stays, ticked, for the World to refuse by name.
            for machine in &initial {
                if first && !every.contains(machine) {
                    options.push((machine.to_string(), true));
                }
            }
        }
    }

    fn done(&mut self, ctx: &Ctx, done: Done) -> Act {
        let words = &ctx.words;
        match (&self.kind, done) {
            (Kind::OpenAgent { .. }, Done::Opened(said)) => {
                let here =
                    self.nodes.as_ref().map(|nodes| nodes.machine.clone()).unwrap_or_else(|| said.to.machine.clone());
                let project = self.chosen_project_name();
                let agent = self.value(Key::Agent);
                let first = self.value(Key::FirstWords);
                Act::Replace(Page::Talk(Box::new(talk::TalkPage::opened(
                    said.to.clone(),
                    agent,
                    project,
                    first,
                    said,
                    here,
                ))))
            }
            (Kind::NewProject, Done::Record(record)) => {
                Act::Replace(Page::Project(Box::new(project::ProjectPage::new(record.town))))
            }
            (Kind::EditProject { .. } | Kind::Revoke { .. } | Kind::Leave, _) => Act::Back,
            (Kind::Invite, Done::Code { code, expires }) => {
                let look = &ctx.look;
                self.result = Some(vec![
                    Line::from(Span::styled(code, look.text(Role::Accent).add_modifier(Modifier::BOLD))),
                    Line::default(),
                    Line::from(Span::styled(words.invite_how(), look.text(Role::Text))),
                    Line::from(Span::styled(
                        words.invite_until(&crate::words::clock(expires, ctx.now)),
                        look.text(Role::Muted),
                    )),
                ]);
                Act::Stay
            }
            (Kind::Join, Done::Joined { world, name }) => {
                self.result = Some(vec![Line::from(Span::styled(
                    words.joined(&world, name.as_str()),
                    ctx.look.text(Role::Text),
                ))]);
                Act::Stay
            }
            _ => Act::Stay,
        }
    }

    /// What finishing the form does, in a word.
    fn kind_word(&self) -> crate::words::Finish {
        use crate::words::Finish;
        match self.kind {
            Kind::OpenAgent { .. } => Finish::Open,
            Kind::NewProject => Finish::Create,
            Kind::EditProject { .. } => Finish::Save,
            Kind::Invite => Finish::Invite,
            Kind::Join => Finish::Join,
            Kind::Revoke { .. } => Finish::Revoke,
            Kind::Leave => Finish::Leave,
        }
    }

    fn chosen_project_name(&self) -> Option<String> {
        let town = self.value(Key::Project);
        self.towns.iter().find(|summary| summary.town.as_str() == town).map(|summary| summary.record.name.clone())
    }

    pub fn key(&mut self, ctx: &Ctx, key: KeyEvent) -> Act {
        if key.code == KeyCode::Esc {
            return Act::Back;
        }
        if self.result.is_some() || self.working {
            return Act::Stay;
        }
        if self.fields.is_empty() {
            return if key.code == KeyCode::Enter { self.submit(ctx) } else { Act::Stay };
        }
        let last = self.fields.len() - 1;
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Tab if !shift => {
                self.focus = (self.focus + 1).min(last);
                return Act::Stay;
            }
            KeyCode::BackTab | KeyCode::Tab => {
                self.focus = self.focus.saturating_sub(1);
                return Act::Stay;
            }
            _ => {}
        }
        let focus = self.focus;
        let field = &mut self.fields[focus];
        let moved = match &mut field.value {
            Value::Text(editor) => match editor.key(&key) {
                Edit::Enter => return if focus == last { self.submit(ctx) } else { self.next() },
                Edit::Unhandled => key.code,
                _ => return Act::Stay,
            },
            Value::Choice { options, index } => match key.code {
                KeyCode::Left => {
                    *index = index.saturating_sub(1);
                    KeyCode::Null
                }
                KeyCode::Right => {
                    *index = (*index + 1).min(options.len().saturating_sub(1));
                    KeyCode::Null
                }
                KeyCode::Enter => return if focus == last { self.submit(ctx) } else { self.next() },
                other => other,
            },
            Value::Ticks { options, at } => match key.code {
                KeyCode::Left => {
                    *at = at.saturating_sub(1);
                    return Act::Stay;
                }
                KeyCode::Right => {
                    *at = (*at + 1).min(options.len().saturating_sub(1));
                    return Act::Stay;
                }
                KeyCode::Char(' ') => {
                    if let Some((_, on)) = options.get_mut(*at) {
                        *on = !*on;
                    }
                    return Act::Stay;
                }
                KeyCode::Enter => return if focus == last { self.submit(ctx) } else { self.next() },
                other => other,
            },
        };
        match moved {
            // The machine chosen decides which projects can be chosen.
            KeyCode::Null if self.fields[focus].key == Key::Machine => self.refresh_choices(ctx),
            KeyCode::Up => self.focus = self.focus.saturating_sub(1),
            KeyCode::Down => self.focus = (self.focus + 1).min(last),
            _ => {}
        }
        Act::Stay
    }

    fn next(&mut self) -> Act {
        self.focus = (self.focus + 1).min(self.fields.len().saturating_sub(1));
        Act::Stay
    }

    fn submit(&mut self, ctx: &Ctx) -> Act {
        match self.job(ctx) {
            Ok(job) => {
                self.working = true;
                self.note = Some(quiet(ctx, ctx.words.working()));
                Act::Do(job)
            }
            Err(why) => {
                self.note = Some(wrong(ctx, why));
                Act::Stay
            }
        }
    }

    /// What the answers ask of the World, or what is wrong with them.
    fn job(&self, ctx: &Ctx) -> Result<Job, String> {
        let words = &ctx.words;
        let optional_machine = |text: String| -> Result<Option<MachineName>, String> {
            if text.is_empty() {
                Ok(None)
            } else {
                MachineName::try_from(text).map(Some).map_err(|error| error.to_string())
            }
        };
        Ok(match &self.kind {
            Kind::OpenAgent { .. } => {
                let text = self.value(Key::FirstWords);
                if text.is_empty() {
                    return Err(words.say_something_first().to_string());
                }
                let agent = AgentName::try_from(self.value(Key::Agent)).map_err(|error| error.to_string())?;
                let town = self.value(Key::Project);
                let town = if town.is_empty() {
                    None
                } else {
                    Some(TownName::try_from(town).map_err(|error| error.to_string())?)
                };
                let node = optional_machine(self.value(Key::Machine))?;
                Job::Open { agent, town, node, text }
            }
            Kind::NewProject => {
                let town = TownName::try_from(self.value(Key::Slug)).map_err(|error| error.to_string())?;
                Job::OpenTown { town, fields: self.record_fields(ctx)? }
            }
            Kind::EditProject { town, .. } => Job::EditTown { town: town.clone(), fields: self.record_fields(ctx)? },
            Kind::Invite => {
                let hours: u32 = self.value(Key::Hours).parse().map_err(|_| words.hours_wrong().to_string())?;
                Job::Invite { name: optional_machine(self.value(Key::InviteName))?, hours }
            }
            Kind::Join => {
                let code = self.value(Key::Code);
                if code.is_empty() {
                    return Err(words.code_needed().to_string());
                }
                Job::Join { code, name: optional_machine(self.value(Key::JoinName))? }
            }
            Kind::Revoke { name } => Job::Revoke { name: name.clone(), reason: self.value(Key::Reason) },
            Kind::Leave => Job::Leave,
        })
    }

    fn record_fields(&self, ctx: &Ctx) -> Result<RecordFields, String> {
        let words = &ctx.words;
        let name = self.value(Key::Name);
        let what = self.value(Key::What);
        if name.is_empty() || what.is_empty() {
            return Err(words.name_and_what_needed().to_string());
        }
        let machines: Vec<MachineName> = match self.fields.iter().find(|field| field.key == Key::Machines) {
            Some(Field { value: Value::Ticks { options, .. }, .. }) => options
                .iter()
                .filter(|(_, on)| *on)
                .map(|(name, _)| MachineName::try_from(name.as_str()).map_err(|error| error.to_string()))
                .collect::<Result<_, _>>()?,
            _ => Vec::new(),
        };
        let mut places = Vec::new();
        for line in self.value(Key::Places).lines().map(str::trim).filter(|line| !line.is_empty()) {
            let parts: Vec<&str> = line.splitn(3, '|').map(str::trim).collect();
            let [what, machine, path] = parts[..] else {
                return Err(words.place_wrong(line));
            };
            let machine = MachineName::try_from(machine).map_err(|error| error.to_string())?;
            places.push(Listing { what: what.to_string(), location: Location { machine, path: path.to_string() } });
        }
        let canon = match self.value(Key::Canon) {
            canon if canon.is_empty() => None,
            canon => {
                let Some((machine, path)) = canon.split_once('|') else {
                    return Err(words.canon_wrong(&canon));
                };
                let machine = MachineName::try_from(machine.trim()).map_err(|error| error.to_string())?;
                Some(Location { machine, path: path.trim().to_string() })
            }
        };
        Ok(RecordFields { name: Some(name), what: Some(what), machines: Some(machines), places: Some(places), canon })
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect, pasting: Option<usize>) {
        let words = &ctx.words;
        let look = &ctx.look;
        let parts = paint::frame(area);
        let width = parts.body.width as usize;
        let title = match &self.kind {
            Kind::OpenAgent { .. } => words.form_open_agent().to_string(),
            Kind::NewProject => words.form_new_project().to_string(),
            Kind::EditProject { town, .. } => words.form_edit_project(town.as_str()),
            Kind::Invite => words.form_invite().to_string(),
            Kind::Join => words.form_join().to_string(),
            Kind::Revoke { name } => words.form_revoke(name.as_str()),
            Kind::Leave => words.form_leave().to_string(),
        };
        let mut rows: Vec<Row> = Vec::new();
        let mut caret: Option<(usize, usize)> = None;
        if let Some(result) = &self.result {
            rows.extend(result.iter().cloned().map(Row::new));
        } else {
            match &self.kind {
                Kind::Revoke { name } => {
                    for line in text::wrap_plain(&words.revoke_warning(name.as_str()), look.text(Role::Caution), width)
                    {
                        rows.push(Row::new(line));
                    }
                    rows.push(Row::blank());
                }
                Kind::Leave => {
                    for line in text::wrap_plain(words.leave_warning(), look.text(Role::Caution), width) {
                        rows.push(Row::new(line));
                    }
                    rows.push(Row::blank());
                    rows.push(Row::new(quiet(ctx, words.enter_to_confirm())));
                }
                _ => {}
            }
            let label_width =
                self.fields.iter().map(|field| text::width(words.field(field.key))).max().unwrap_or(0) + 3;
            let value_width = width.saturating_sub(label_width).max(8);
            for (index, field) in self.fields.iter().enumerate() {
                let focused = index == self.focus;
                let label_style =
                    if focused { look.text(Role::Text).add_modifier(Modifier::BOLD) } else { look.text(Role::Muted) };
                let label = words.field(field.key);
                let pad = " ".repeat(label_width - text::width(label));
                let mut value_lines: Vec<Line<'static>> = match &field.value {
                    Value::Text(editor) => {
                        let (lines, at) = editor.layout(value_width, look.text(Role::Text));
                        if focused {
                            caret = Some((rows.len() + at.0, label_width + at.1));
                        }
                        if editor.text().is_empty() && !focused {
                            vec![quiet(ctx, "—")]
                        } else {
                            let most =
                                if matches!(field.key, Key::FirstWords | Key::Places | Key::What) { 8 } else { 1 };
                            lines.into_iter().take(most.max(at.0 + 1)).collect()
                        }
                    }
                    Value::Choice { options, index } => {
                        let mut spans = Vec::new();
                        if options.is_empty() {
                            spans.push(Span::styled(words.asking(), look.text(Role::Faint)));
                        }
                        for (at, (shown, _)) in options.iter().enumerate() {
                            if at > 0 {
                                spans.push(Span::raw("    "));
                            }
                            if at == *index {
                                spans.push(Span::styled("‹ ", look.text(Role::Accent)));
                                spans.push(Span::styled(
                                    shown.clone(),
                                    look.text(Role::Text).add_modifier(Modifier::BOLD),
                                ));
                                spans.push(Span::styled(" ›", look.text(Role::Accent)));
                            } else {
                                spans.push(Span::styled(shown.clone(), look.text(Role::Faint)));
                            }
                        }
                        vec![Line::from(spans)]
                    }
                    Value::Ticks { options, at } => {
                        let mut spans = Vec::new();
                        for (index, (name, on)) in options.iter().enumerate() {
                            if index > 0 {
                                spans.push(Span::raw("    "));
                            }
                            let mark = if *on {
                                Span::styled("● ", look.text(Role::Accent))
                            } else {
                                Span::styled("○ ", look.text(Role::Faint))
                            };
                            spans.push(mark);
                            let mut style = if *on { look.text(Role::Text) } else { look.text(Role::Muted) };
                            if focused && index == *at {
                                style = style.add_modifier(Modifier::UNDERLINED);
                            }
                            spans.push(Span::styled(name.clone(), style));
                        }
                        vec![Line::from(spans)]
                    }
                };
                if value_lines.is_empty() {
                    value_lines.push(Line::default());
                }
                for (line_index, mut line) in value_lines.into_iter().enumerate() {
                    let lead = if line_index == 0 { format!("{label}{pad}") } else { " ".repeat(label_width) };
                    line.spans.insert(0, Span::styled(lead, label_style));
                    rows.push(Row::new(line).item(index));
                }
                if focused && let Some(hint) = words.hint(field.key) {
                    for line in text::wrap_plain(hint, look.text(Role::Faint), value_width) {
                        let mut line = line;
                        line.spans.insert(0, Span::raw(" ".repeat(label_width)));
                        rows.push(Row::new(line).item(index));
                    }
                }
                rows.push(Row::blank());
            }
        }

        let buffer = frame.buffer_mut();
        let mut crumbs = vec![words.world().to_string()];
        if let Kind::EditProject { town, .. } = &self.kind {
            crumbs.push(town.to_string());
        }
        crumbs.push(title);
        paint::header(buffer, parts.header, look, &crumbs, &Line::default());
        let height = parts.body.height as usize;
        if let Some((first, last)) = paint::item_rows(&rows, self.focus) {
            self.top = paint::keep_in_view(self.top, first, last, height);
        }
        let selected = if self.result.is_none() && !self.fields.is_empty() { Some(self.focus) } else { None };
        paint::rows(buffer, parts.body, &rows, self.top, selected, look);
        let note = match pasting {
            Some(count) => Some(quiet(ctx, words.pasting(count))),
            None => self.note.clone(),
        };
        if let Some(note) = note {
            paint::line(buffer, parts.note.x, parts.note.y, parts.note.width, &note, &Line::default(), false, look);
        }
        let keys: Vec<(&str, &str)> = if self.result.is_some() {
            vec![("Esc", words.key_back())]
        } else if self.fields.is_empty() {
            vec![("Enter", words.key_confirm()), ("Esc", words.key_cancel())]
        } else {
            let mut keys = vec![("Tab", words.key_next_field())];
            if self.fields.iter().any(|field| matches!(field.value, Value::Choice { .. } | Value::Ticks { .. })) {
                keys.push(("← →", words.key_choose()));
            }
            if self.fields.iter().any(|field| matches!(field.value, Value::Ticks { .. })) {
                keys.push(("Space", words.key_tick()));
            }
            let last = self.focus + 1 == self.fields.len();
            keys.push(("Enter", if last { words.key_finish(&self.kind_word()) } else { words.key_next_field() }));
            keys.push(("Esc", words.key_cancel()));
            keys
        };
        paint::footer(buffer, parts.footer, look, &keys);
        if let Some((row, column)) = caret
            && row >= self.top
            && row < self.top + height
            && self.result.is_none()
        {
            frame.set_cursor_position(Position::new(
                parts.body.x + column as u16,
                parts.body.y + (row - self.top) as u16,
            ));
        }
    }
}
