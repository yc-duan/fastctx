//! Asking the World for something that takes a few answers: a conversation
//! with a new agent, a project opened or its record written anew, logging
//! this computer in to an account or out of one, adding another device, a
//! member taken out of an account. Choices come from what the
//! World says now: the machines online, the agents assisting this computer's
//! accounts, the projects those accounts may use, the accounts this
//! computer is logged in to.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fastctx_types::project::{Listing, Location, Record, RecordFields, TownSummary};
use fastctx_types::world::NodesReply;
use fastctx_types::{AccountName, AgentName, HubName, MachineName, TownName};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use super::{Page, project, quiet, talk, wrong};
use crate::app::{Act, Assisting, Ctx, Data, Done, Feed, Job};
use crate::design::Role;
use crate::editor::{Edit, Editor};
use crate::paint::{self, Row};
use crate::text;
use crate::words::FieldName as Key;

/// How a machine choice names an agent assisting this computer's accounts:
/// this, then its place among them.
const ASSISTING: &str = "assisting:";

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
    assisting: Vec<Assisting>,
    top: usize,
    /// How many times a new project was asked for: after the first, its
    /// short name takes an ending of its own, the one tried being taken.
    tries: u32,
}

enum Kind {
    OpenAgent { preset: Option<TownName> },
    NewProject,
    EditProject { town: TownName, machines: Vec<MachineName> },
    Login,
    Device,
    Logout,
    Remove { hub: HubName, account: AccountName, name: MachineName },
}

struct Field {
    key: Key,
    value: Value,
    /// Shown as dots: a password.
    secret: bool,
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
        Self { key, value: Value::Text(editor), secret: false }
    }

    fn secret(key: Key) -> Self {
        Self { key, value: Value::Text(Editor::single()), secret: true }
    }

    fn choice(key: Key, options: Vec<(String, String)>) -> Self {
        Self { key, value: Value::Choice { options, index: 0 }, secret: false }
    }

    fn ticks(key: Key) -> Self {
        Self { key, value: Value::Ticks { options: Vec::new(), at: 0 }, secret: false }
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
            assisting: Vec::new(),
            top: 0,
            tries: 0,
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
                Field::text(Key::Name, Editor::single()),
                Field::text(Key::What, Editor::multi()),
                Field::ticks(Key::Machines),
                Field::text(Key::Places, Editor::multi()),
            ],
        )
    }

    pub fn edit_project(town: TownName, record: &Record) -> Self {
        let places: Vec<String> = record
            .places
            .iter()
            .map(|place| format!("{} | {} | {}", place.what, place.location.machine, place.location.path))
            .collect();
        Self::new(
            Kind::EditProject { town, machines: record.machines.clone() },
            vec![
                Field::text(Key::Name, Editor::single().with(&record.name)),
                Field::text(Key::What, Editor::multi().with(&record.what)),
                Field::ticks(Key::Machines),
                Field::text(Key::Places, Editor::multi().with(&places.join("\n"))),
            ],
        )
    }

    /// Logs this computer in with the hub's address and an account's
    /// password, which joins it as one of the account's machines; `first`
    /// asks what it is called, as it is on no hub yet.
    pub fn login(_ctx: &Ctx, first: bool) -> Self {
        let mut fields = vec![
            Field::text(Key::HubAddress, Editor::single()),
            Field::text(Key::Account, Editor::single()),
            Field::secret(Key::Password),
        ];
        if first {
            fields.push(Field::text(Key::LoginName, Editor::single()));
        }
        Self::new(Kind::Login, fields)
    }

    /// Adds another device to one of `logins`, the accounts this computer
    /// is logged in to: a computer is given the hub's address and the
    /// account, a Linux server the line that installs it. A phone scans the
    /// code the graphical interface shows.
    pub fn device(ctx: &Ctx, logins: &[(HubName, AccountName)], preset: Option<&(HubName, AccountName)>) -> Self {
        let words = &ctx.words;
        let mut kinds = vec![(words.device_computer().to_string(), "computer".to_string())];
        kinds.extend([1u32, 24, 72].iter().map(|hours| (words.device_server(*hours), format!("server:{hours}"))));
        let mut accounts = Field::choice(Key::HubAccount, account_options(logins));
        if let (Some(preset), Value::Choice { index, .. }) = (preset, &mut accounts.value) {
            *index = logins.iter().position(|login| login == preset).unwrap_or(0);
        }
        Self::new(Kind::Device, vec![accounts, Field::choice(Key::DeviceKind, kinds)])
    }

    /// Logs this computer out of one of `logins`.
    pub fn logout(logins: &[(HubName, AccountName)], preset: Option<&(HubName, AccountName)>) -> Self {
        let mut accounts = Field::choice(Key::HubAccount, account_options(logins));
        if let (Some(preset), Value::Choice { index, .. }) = (preset, &mut accounts.value) {
            *index = logins.iter().position(|login| login == preset).unwrap_or(0);
        }
        Self::new(Kind::Logout, vec![accounts])
    }

    /// Takes `name` out of `account`: a device's login of it, or one of its
    /// machines.
    pub fn remove(hub: HubName, account: AccountName, name: MachineName) -> Self {
        Self::new(Kind::Remove { hub, account, name }, vec![Field::text(Key::Reason, Editor::single())])
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

    /// What was typed, untrimmed: a password is what it is.
    fn raw(&self, key: Key) -> String {
        match self.fields.iter().find(|field| field.key == key).map(|field| &field.value) {
            Some(Value::Text(editor)) => editor.text().to_string(),
            _ => String::new(),
        }
    }

    pub fn paste(&mut self, text: String) -> Act {
        if let Some(Field { value: Value::Text(editor), .. }) = self.fields.get_mut(self.focus) {
            editor.insert(&text);
        }
        Act::Stay
    }

    pub fn data(&mut self, ctx: &Ctx, data: Data) -> Act {
        match data {
            Data::Choices { nodes, towns, assisting } => {
                if let Ok(nodes) = nodes {
                    self.nodes = Some(nodes);
                }
                self.assisting = assisting;
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
        // The machines whose agents this computer's accounts may use, the
        // online ones, and every machine of its hubs.
        let mut usable = vec![here.clone()];
        let mut every = vec![here.clone()];
        for node in nodes.hubs.iter().flat_map(|hub| &hub.machines) {
            if node.online != Some(false)
                && node.rights.iter().any(|(_, rights)| rights.agents)
                && !usable.contains(&node.name)
            {
                usable.push(node.name.clone());
            }
            if !every.contains(&node.name) {
                every.push(node.name.clone());
            }
        }
        let towns = self.towns.clone();
        let self_assisting = self.assisting.clone();
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
            // An agent assisting one of this computer's accounts lives on a
            // machine of another's, which it works for them from.
            options.extend(self_assisting.iter().enumerate().map(|(at, assisting)| {
                (
                    words.assisting_choice(
                        assisting.machine.as_str(),
                        assisting.owner.as_ref().map(|owner| owner.as_str()),
                        assisting.agent.as_str(),
                    ),
                    format!("{ASSISTING}{at}"),
                )
            }));
            *index = options.iter().position(|(_, value)| *value == chosen).unwrap_or(0);
        }
        if let Some(Field { value: Value::Choice { options, index }, .. }) = self.field(Key::Project) {
            let chosen = options.get(*index).map(|(_, value)| value.clone()).or(preset.map(|town| town.to_string()));
            let mut list = vec![(words.no_project().to_string(), String::new())];
            list.extend(towns.iter().map(|town| (town.record.name.clone(), town.town.to_string())));
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
                let town = self.chosen_town();
                let agent = self.value(Key::Agent);
                let first = self.value(Key::FirstWords);
                Act::Replace(Page::Talk(Box::new(talk::TalkPage::opened(agent, town, project, first, said, here))))
            }
            (Kind::NewProject, Done::Record(record)) => {
                Act::Replace(Page::Project(Box::new(project::ProjectPage::new(record.town))))
            }
            (Kind::EditProject { .. } | Kind::Logout | Kind::Remove { .. }, _) => Act::Back,
            (Kind::Device, Done::ServerLine { text, expires }) => {
                let look = &ctx.look;
                self.result = Some(vec![
                    Line::from(Span::styled(words.server_how(), look.text(Role::Muted))),
                    Line::default(),
                    Line::from(Span::styled(text, look.text(Role::Accent).add_modifier(Modifier::BOLD))),
                    Line::default(),
                    Line::from(Span::styled(
                        words.server_until(&crate::words::clock(expires, ctx.now)),
                        look.text(Role::Muted),
                    )),
                ]);
                Act::Stay
            }
            (Kind::Device, Done::Device { addresses, account }) => {
                let look = &ctx.look;
                let fact = |name: &str, value: String| {
                    Line::from(vec![
                        Span::styled(format!("{name}  "), look.text(Role::Muted)),
                        Span::styled(value, look.text(Role::Accent).add_modifier(Modifier::BOLD)),
                    ])
                };
                self.result = Some(vec![
                    fact(words.field(Key::HubAddress), addresses.join("  ")),
                    fact(words.field(Key::Account), account.to_string()),
                    Line::default(),
                    Line::from(Span::styled(words.computer_how(), look.text(Role::Muted))),
                ]);
                Act::Stay
            }
            (Kind::Login, Done::LoggedIn { hub, name, account, kept }) => {
                let (said, role) = match &kept {
                    Ok(kept) => (words.kept(kept), Role::Muted),
                    Err(why) => (words.not_kept(why), Role::Caution),
                };
                self.result = Some(vec![
                    Line::from(Span::styled(
                        words.logged_in(hub.as_str(), name.as_str(), account.as_str()),
                        ctx.look.text(Role::Text),
                    )),
                    Line::from(Span::styled(said, ctx.look.text(role))),
                ]);
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
            Kind::Login => Finish::Login,
            Kind::Device => Finish::Device,
            Kind::Logout => Finish::Logout,
            Kind::Remove { .. } => Finish::Remove,
        }
    }

    fn chosen_town(&self) -> Option<TownName> {
        let town = self.value(Key::Project);
        self.towns.iter().find(|summary| summary.town.as_str() == town).map(|summary| summary.town.clone())
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
        let job = self.job(ctx);
        if matches!(self.kind, Kind::NewProject) {
            self.tries += 1;
        }
        match job {
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
            if text.is_empty() { Ok(None) } else { machine_named(ctx, &text).map(Some) }
        };
        Ok(match &self.kind {
            Kind::OpenAgent { .. } => {
                let text = self.value(Key::FirstWords);
                if text.is_empty() {
                    return Err(words.say_something_first().to_string());
                }
                let town = self.value(Key::Project);
                let town = if town.is_empty() {
                    None
                } else {
                    Some(TownName::try_from(town).map_err(|error| error.to_string())?)
                };
                let machine = self.value(Key::Machine);
                // An assisting agent is that agent, in the login that
                // assists, for the account it assists.
                if let Some(assisting) = machine
                    .strip_prefix(ASSISTING)
                    .and_then(|at| at.parse::<usize>().ok())
                    .and_then(|at| self.assisting.get(at))
                {
                    let choices = fastctx_types::agents::Choices {
                        account: Some(assisting.to.clone()),
                        home: assisting.home.clone(),
                        ..Default::default()
                    };
                    return Ok(Job::Open {
                        agent: assisting.agent.clone(),
                        town,
                        node: Some(assisting.machine.clone()),
                        text,
                        choices,
                    });
                }
                let agent = AgentName::try_from(self.value(Key::Agent)).map_err(|error| error.to_string())?;
                let node = optional_machine(machine)?;
                Job::Open { agent, town, node, text, choices: Default::default() }
            }
            Kind::NewProject => {
                let town = short_name(&self.value(Key::Name), self.tries);
                Job::OpenTown { town, fields: self.record_fields(ctx)? }
            }
            Kind::EditProject { town, .. } => Job::EditTown { town: town.clone(), fields: self.record_fields(ctx)? },
            Kind::Login => {
                let hub = self.value(Key::HubAddress);
                let account = self.value(Key::Account);
                let password = self.raw(Key::Password);
                if hub.is_empty() || account.is_empty() || password.is_empty() {
                    return Err(words.login_needs().to_string());
                }
                let account = AccountName::try_from(account).map_err(|error| error.to_string())?;
                Job::Login {
                    hub,
                    account,
                    password,
                    register: true,
                    name: optional_machine(self.value(Key::LoginName))?,
                }
            }
            Kind::Device => {
                let (hub, account) = hub_account(&self.value(Key::HubAccount))?;
                match self.value(Key::DeviceKind).strip_prefix("server:") {
                    Some(hours) => Job::ServerLine { hub, account, hours: hours.parse().unwrap_or(24) },
                    None => Job::Device { hub, account },
                }
            }
            Kind::Logout => {
                let (hub, account) = hub_account(&self.value(Key::HubAccount))?;
                Job::Logout { hub, account }
            }
            Kind::Remove { hub, account, name } => Job::Remove {
                hub: hub.clone(),
                account: account.clone(),
                name: name.clone(),
                reason: self.value(Key::Reason),
                seen: false,
            },
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
                .map(|(name, _)| machine_named(ctx, name))
                .collect::<Result<_, _>>()?,
            _ => Vec::new(),
        };
        let mut places = Vec::new();
        for line in self.value(Key::Places).lines().map(str::trim).filter(|line| !line.is_empty()) {
            let parts: Vec<&str> = line.splitn(3, '|').map(str::trim).collect();
            let [what, machine, path] = parts[..] else {
                return Err(words.place_wrong(line));
            };
            let machine = machine_named(ctx, machine)?;
            places.push(Listing { what: what.to_string(), location: Location { machine, path: path.to_string() } });
        }
        let canon = match self.value(Key::Canon) {
            canon if canon.is_empty() => None,
            canon => {
                let Some((machine, path)) = canon.split_once('|') else {
                    return Err(words.canon_wrong(&canon));
                };
                let machine = machine_named(ctx, machine.trim())?;
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
            Kind::Login => words.form_login().to_string(),
            Kind::Device => words.form_device().to_string(),
            Kind::Logout => words.form_logout().to_string(),
            Kind::Remove { name, account, .. } => words.form_remove(name.as_str(), account.as_str()),
        };
        let mut rows: Vec<Row> = Vec::new();
        let mut caret: Option<(usize, usize)> = None;
        if let Some(result) = &self.result {
            rows.extend(result.iter().cloned().map(Row::new));
        } else {
            match &self.kind {
                Kind::Remove { name, account, .. } => {
                    let warning = words.remove_warning(name.as_str(), account.as_str());
                    for line in text::wrap_plain(&warning, look.text(Role::Caution), width) {
                        rows.push(Row::new(line));
                    }
                    rows.push(Row::blank());
                }
                Kind::Logout => {
                    for line in text::wrap_plain(words.logout_warning(), look.text(Role::Caution), width) {
                        rows.push(Row::new(line));
                    }
                    rows.push(Row::blank());
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
                    Value::Text(editor) if field.secret => {
                        let dots = "•".repeat(editor.text().chars().count());
                        if focused {
                            caret = Some((rows.len(), label_width + text::width(&dots)));
                        }
                        vec![Line::from(Span::styled(dots, look.text(Role::Text)))]
                    }
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

/// `text` as a machine name, or what is wrong with it in the user's words.
fn machine_named(ctx: &Ctx, text: &str) -> Result<MachineName, String> {
    MachineName::try_from(text).map_err(|error| match &error.problem {
        Some(problem) => ctx.words.name_problem(problem),
        None => error.to_string(),
    })
}

/// The accounts to choose from, each shown as `account · hub`.
fn account_options(logins: &[(HubName, AccountName)]) -> Vec<(String, String)> {
    logins.iter().map(|(hub, account)| (format!("{account} · {hub}"), format!("{hub}/{account}"))).collect()
}

fn hub_account(value: &str) -> Result<(HubName, AccountName), String> {
    let (hub, account) = value.split_once('/').ok_or_else(|| "choose an account".to_string())?;
    let hub = HubName::try_from(hub).map_err(|error| error.to_string())?;
    let account = AccountName::try_from(account).map_err(|error| error.to_string())?;
    Ok((hub, account))
}

/// A new project's short name, which commands and each machine's folders
/// know it by and the user never types: its name's own words where it is
/// written in Latin letters, else `project`; with an ending of its own once
/// a try found the name taken. At most 32 characters, as every release
/// reads them.
fn short_name(name: &str, tries: u32) -> TownName {
    let mut words = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || (c.is_ascii_digit() && !words.is_empty()) {
            words.push(c);
        } else if !words.is_empty() && !words.ends_with('-') {
            words.push('-');
        }
        if words.len() >= 24 {
            break;
        }
    }
    let words = words.trim_end_matches('-');
    let base = if words.is_empty() { "project" } else { words };
    if tries == 0
        && base != "project"
        && let Ok(town) = TownName::try_from(base)
    {
        return town;
    }
    const LETTERS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut seed = jiff::Timestamp::now().as_nanosecond().unsigned_abs() as u64 ^ u64::from(tries).rotate_left(17);
    let ending: String = (0..4)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            LETTERS[(seed >> 59) as usize] as char
        })
        .collect();
    TownName::try_from(format!("{base}-{ending}")).expect("letters, digits and hyphens make a short name")
}
