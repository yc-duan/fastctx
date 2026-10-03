//! World forms retain drafts across validation and operation failures.
//!
//! A form is a list of fields plus two pseudo-targets: the advanced-options toggle, which hides
//! the settings a first-time user never needs, and the submit button. Focus moves over the
//! visible targets only, and a validation failure in a hidden field opens the section it is in.

use super::i18n::WorldMessages;
use crate::tui::text_input::{InputMode, TextInput};
use crate::world::control::{Action, Failure};
use crate::world::grant::{Grant, GrantField};
use crate::world::{NetworkMode, notes};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(super) enum FormKind {
    Create,
    Join,
    Invite,
    Description,
    PrivateNote(String),
    Project,
    Grant(u64),
    Cursor,
    HubDirectory,
    Export(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FieldKind {
    Name,
    Hub,
    Bootstrap,
    Invite,
    Tags,
    Network,
    Interface,
    AutoService,
    Text,
    Hours,
    Hubs,
    Principal,
    Targets,
    Verbs,
    Expires,
    Cursor,
    Directory,
    Path,
    Town,
    What,
    Machines,
    Places,
    Canon,
}

impl FieldKind {
    pub(super) fn label(self, m: &WorldMessages, form: &FormKind) -> &'static str {
        match self {
            Self::Name if matches!(form, FormKind::Project) => m.project_name,
            Self::Name => m.name,
            Self::Hub => m.hub_address,
            Self::Bootstrap => m.bootstrap,
            Self::Invite => m.invite_string,
            Self::Tags => m.tags,
            Self::Network => m.network,
            Self::Interface => m.interface,
            Self::AutoService => m.auto_service,
            Self::Text => {
                if matches!(form, FormKind::Description) {
                    m.description
                } else {
                    m.private_note
                }
            }
            Self::Hours => m.hours,
            Self::Hubs => m.hub_address,
            Self::Principal => m.principal,
            Self::Targets => m.targets,
            Self::Verbs => m.verbs,
            Self::Expires => m.expires,
            Self::Cursor => m.cursor,
            Self::Directory => m.directory,
            Self::Path => m.path,
            Self::Town => m.town_slug,
            Self::What => m.project_what,
            Self::Machines => m.machines,
            Self::Places => m.places,
            Self::Canon => m.canon,
        }
    }
    pub(super) fn help(self, m: &WorldMessages, form: &FormKind) -> &'static str {
        match self {
            Self::Name if matches!(form, FormKind::Project) => m.project_name_help,
            Self::Name => m.name_help,
            Self::Hub => m.hub_field_help,
            Self::Bootstrap => m.bootstrap_help,
            Self::Invite => m.join_help,
            Self::Tags => m.tags_help,
            Self::Network => m.network_help,
            Self::Interface => m.interface_help,
            Self::AutoService => m.auto_service_help,
            Self::Text => {
                if matches!(form, FormKind::Description) {
                    m.description_help
                } else {
                    m.note_help
                }
            }
            Self::Hours => m.hours_help,
            Self::Hubs => m.hubs_help,
            Self::Principal => m.principal_help,
            Self::Targets => m.targets_help,
            Self::Verbs => m.verbs_help,
            Self::Expires => m.expires_help,
            Self::Cursor => m.cursor_help,
            Self::Directory => m.hub_help,
            Self::Path => m.path_help,
            Self::Town => m.town_slug_help,
            Self::What => m.project_what_help,
            Self::Machines => m.machines_help,
            Self::Places => m.places_help,
            Self::Canon => m.canon_help,
        }
    }
    /// A field chosen with Left/Right rather than typed.
    pub(super) fn is_choice(self) -> bool {
        matches!(self, Self::Network | Self::AutoService)
    }
    /// A field the enrollment forms keep behind the advanced toggle.
    pub(super) fn is_advanced(self) -> bool {
        matches!(
            self,
            Self::Tags | Self::Network | Self::Interface | Self::AutoService
        )
    }
    /// Submission refuses an empty value here.
    pub(super) fn required(self) -> bool {
        matches!(
            self,
            Self::Name
                | Self::Hub
                | Self::Bootstrap
                | Self::Invite
                | Self::Hours
                | Self::Principal
                | Self::Targets
                | Self::Verbs
                | Self::Cursor
                | Self::Directory
                | Self::Path
                | Self::Town
                | Self::What
                | Self::Machines
        )
    }
}

#[derive(Clone, Debug)]
pub(super) struct Field {
    pub(super) kind: FieldKind,
    pub(super) input: TextInput,
    original: String,
}

impl Field {
    fn new(kind: FieldKind, value: &str, limit: usize, mode: InputMode, secret: bool) -> Self {
        Self {
            kind,
            input: TextInput::new(value.to_string(), limit, mode, secret),
            original: value.to_string(),
        }
    }
}

/// One thing the cursor can rest on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Focus {
    Field(usize),
    Advanced,
    Submit,
}

#[derive(Clone, Debug)]
pub(super) struct Form {
    pub(super) kind: FormKind,
    pub(super) fields: Vec<Field>,
    /// Index into `targets()`.
    selected: usize,
    pub(super) advanced: bool,
    pub(super) error: Option<Failure>,
    pub(super) error_hint: Option<ErrorHint>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ErrorHint {
    Field(FieldKind),
    Choice,
    IncompletePaste,
}

pub(super) enum Submission {
    Action(Action),
    Cursor(u64),
    HubDirectory(PathBuf),
}
pub(super) enum FormEvent {
    None,
    Submit,
    Back,
}

impl Form {
    pub(super) fn new(kind: FormKind, initial: &str) -> Self {
        use FieldKind as F;
        use InputMode as I;
        let field = |kind, value, limit| Field::new(kind, value, limit, I::SingleLine, false);
        let mut fields = match &kind {
            FormKind::Create => vec![
                field(F::Name, initial, 32),
                field(F::Hub, "", 2048),
                Field::new(F::Bootstrap, "", 4096, I::Token, true),
            ],
            FormKind::Join => vec![
                Field::new(F::Invite, "", 8192, I::Token, true),
                field(F::Name, initial, 32),
            ],
            FormKind::Invite => vec![
                field(F::Name, "", 32),
                field(F::Hours, "24", 4),
                field(F::Hubs, "", 4096),
            ],
            FormKind::Description | FormKind::PrivateNote(_) => vec![Field::new(
                F::Text,
                initial,
                notes::MAX_NOTE_CHARS,
                I::Multiline,
                false,
            )],
            FormKind::Grant(_) => vec![
                field(F::Principal, "", 128),
                field(F::Targets, "", 2048),
                field(F::Verbs, "", 512),
                field(F::Expires, "", 64),
            ],
            FormKind::Project => vec![
                field(F::Town, "", 64),
                field(F::Name, "", 200),
                Field::new(F::What, "", 2000, I::Multiline, false),
                field(F::Machines, initial, 2048),
                Field::new(F::Places, "", 8000, I::Multiline, false),
                field(F::Canon, "", 2048),
            ],
            FormKind::Cursor => vec![field(F::Cursor, initial, 20)],
            FormKind::HubDirectory => vec![field(F::Directory, initial, 4096)],
            FormKind::Export(_) => vec![field(F::Path, initial, 4096)],
        };
        if matches!(kind, FormKind::Create | FormKind::Join) {
            fields.extend([
                field(F::Tags, "", 512),
                field(F::Network, "auto", 6),
                field(F::Interface, "", 256),
                field(F::AutoService, "yes", 3),
            ]);
        }
        Self {
            kind,
            fields,
            selected: 0,
            advanced: false,
            error: None,
            error_hint: None,
        }
    }
    pub(super) fn title<'a>(&self, m: &'a WorldMessages) -> &'a str {
        match self.kind {
            FormKind::Create => m.create,
            FormKind::Join => m.join,
            FormKind::Invite => m.invite,
            FormKind::Description => m.edit_description,
            FormKind::PrivateNote(_) => m.edit_note,
            FormKind::Project => m.open_project,
            FormKind::Grant(_) => m.add_grant,
            FormKind::Cursor => m.event_cursor,
            FormKind::HubDirectory => m.choose_hub,
            FormKind::Export(_) => m.export,
        }
    }
    /// One sentence under the title saying what the form does; None for trivial forms.
    pub(super) fn subtitle<'a>(&self, m: &'a WorldMessages) -> Option<&'a str> {
        match self.kind {
            FormKind::Create => Some(m.create_subtitle),
            FormKind::Join => Some(m.join_subtitle),
            FormKind::Invite => Some(m.invite_subtitle),
            FormKind::Project => Some(m.open_project_subtitle),
            FormKind::Grant(_) => Some(m.grant_subtitle),
            _ => None,
        }
    }
    /// What the submit button says: saving forms save, the rest continue to a review.
    pub(super) fn submit_label<'a>(&self, m: &'a WorldMessages) -> &'a str {
        match self.kind {
            FormKind::Description | FormKind::PrivateNote(_) | FormKind::Export(_) => m.save,
            _ => m.continue_action,
        }
    }
    pub(super) fn has_advanced(&self) -> bool {
        self.fields.iter().any(|field| field.kind.is_advanced())
    }
    /// The focusable things, in Tab order, as currently visible.
    pub(super) fn targets(&self) -> Vec<Focus> {
        let mut targets = self
            .fields
            .iter()
            .enumerate()
            .filter(|(_, field)| self.advanced || !field.kind.is_advanced())
            .map(|(index, _)| Focus::Field(index))
            .collect::<Vec<_>>();
        if self.has_advanced() {
            let position = targets
                .iter()
                .position(|target| {
                    matches!(target, Focus::Field(index) if self.fields[*index].kind.is_advanced())
                })
                .unwrap_or(targets.len());
            targets.insert(position, Focus::Advanced);
        }
        targets.push(Focus::Submit);
        targets
    }
    pub(super) fn focus(&self) -> Focus {
        let targets = self.targets();
        targets[self.selected.min(targets.len() - 1)]
    }
    pub(super) fn focused_field(&self) -> Option<usize> {
        match self.focus() {
            Focus::Field(index) => Some(index),
            _ => None,
        }
    }
    #[cfg(test)]
    pub(super) fn focus_submit(&mut self) {
        self.selected = self.targets().len() - 1;
    }
    fn focus_field(&mut self, index: usize) {
        if self.fields[index].kind.is_advanced() {
            self.advanced = true;
        }
        let targets = self.targets();
        self.selected = targets
            .iter()
            .position(|target| *target == Focus::Field(index))
            .unwrap_or(0);
    }
    pub(super) fn dirty(&self) -> bool {
        self.fields
            .iter()
            .any(|field| field.original != field.input.text())
    }
    pub(super) fn paste(&mut self, value: &str) {
        let Some(index) = self.focused_field() else {
            return;
        };
        let field = &mut self.fields[index];
        if field.kind.is_choice() {
            self.error = Some(Failure::invalid(
                "Use Left/Right or Space to choose this option.",
            ));
            self.error_hint = Some(ErrorHint::Choice);
            return;
        }
        field.input.insert(value);
        self.error = None;
        self.invite_name();
    }
    fn invite_name(&mut self) {
        if !matches!(self.kind, FormKind::Join) {
            return;
        }
        if let Ok(invite) = crate::world::invite::Invite::parse(self.fields[0].input.text())
            && self.fields[1].input.text().is_empty()
            && let Some(name) = &invite.name
        {
            self.fields[1].input.replace(name.clone());
        }
    }
    fn toggle_advanced(&mut self) {
        self.advanced = !self.advanced;
        let targets = self.targets();
        self.selected = targets
            .iter()
            .position(|target| *target == Focus::Advanced)
            .unwrap_or(0);
    }
    fn cycle_choice(&mut self, index: usize) {
        let field = &mut self.fields[index];
        let next = match (field.kind, field.input.text()) {
            (FieldKind::Network, "auto") => "direct",
            (FieldKind::Network, "direct") => "system",
            (FieldKind::Network, _) => "auto",
            (_, "yes") => "no",
            _ => "yes",
        };
        field.input.replace(next.to_string());
    }
    pub(super) fn key(&mut self, key: KeyEvent) -> FormEvent {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            return FormEvent::Back;
        }
        if control && key.code == KeyCode::Char('s') {
            return FormEvent::Submit;
        }
        let targets = self.targets();
        let focus = self.focus();
        match key.code {
            KeyCode::BackTab => {
                self.selected = self.selected.checked_sub(1).unwrap_or(targets.len() - 1);
            }
            KeyCode::Tab => self.selected = (self.selected + 1) % targets.len(),
            KeyCode::Enter => match focus {
                Focus::Submit => return FormEvent::Submit,
                Focus::Advanced => self.toggle_advanced(),
                Focus::Field(_) => {
                    self.invite_name();
                    self.selected = (self.selected + 1).min(targets.len() - 1);
                }
            },
            KeyCode::F(2) => {
                if let Focus::Field(index) = focus {
                    self.fields[index].input.revealed = !self.fields[index].input.revealed;
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if focus == Focus::Advanced => {
                self.toggle_advanced();
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if matches!(focus, Focus::Field(index) if self.fields[index].kind.is_choice()) => {
                if let Focus::Field(index) = focus {
                    self.cycle_choice(index);
                }
            }
            _ => {
                if let Focus::Field(index) = focus
                    && !self.fields[index].kind.is_choice()
                    && self.fields[index].input.handle_key(key)
                {
                    self.error = None;
                }
            }
        }
        FormEvent::None
    }
    fn value(&self, kind: FieldKind) -> &str {
        self.fields
            .iter()
            .find(|field| field.kind == kind)
            .map(|field| field.input.text())
            .unwrap_or("")
    }
    fn list(&self, kind: FieldKind) -> Vec<String> {
        self.value(kind)
            .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect()
    }
    fn optional(&self, kind: FieldKind) -> Option<String> {
        let value = self.value(kind).trim();
        (!value.is_empty()).then(|| value.to_string())
    }
    pub(super) fn submit(&mut self) -> Result<Submission, Failure> {
        match self.validate() {
            Err((kind, error)) => {
                if let Some(index) = self.fields.iter().position(|field| field.kind == kind) {
                    self.focus_field(index);
                }
                self.error = Some(error.clone());
                self.error_hint = Some(ErrorHint::Field(kind));
                Err(error)
            }
            Ok(submission) => {
                self.error = None;
                self.error_hint = None;
                Ok(submission)
            }
        }
    }
    pub(super) fn error_text(&self, m: &WorldMessages) -> String {
        let text = match self.error_hint {
            Some(ErrorHint::IncompletePaste) => m.paste_incomplete,
            Some(ErrorHint::Choice) => m.choice_help,
            Some(ErrorHint::Field(kind)) if self.value(kind).is_empty() => m.required,
            Some(ErrorHint::Field(FieldKind::Invite))
                if crate::world::invite::Invite::parse(self.value(FieldKind::Invite))
                    .is_ok_and(|invite| invite.is_expired_at(time::OffsetDateTime::now_utc())) =>
            {
                m.invite_expired
            }
            Some(ErrorHint::Field(FieldKind::Directory | FieldKind::Path)) => m.absolute_path,
            Some(ErrorHint::Field(FieldKind::Cursor)) => m.integer_required,
            Some(ErrorHint::Field(FieldKind::Tags)) => m.name_help,
            Some(ErrorHint::Field(kind)) => kind.help(m, &self.kind),
            None => m.validation,
        };
        text.to_string()
    }
    /// The field the current error belongs to, so it can be shown in place.
    pub(super) fn error_field(&self) -> Option<FieldKind> {
        match self.error_hint {
            Some(ErrorHint::Field(kind)) => Some(kind),
            Some(ErrorHint::Choice) | Some(ErrorHint::IncompletePaste) => {
                self.focused_field().map(|index| self.fields[index].kind)
            }
            None => None,
        }
    }
    fn validate(&self) -> Result<Submission, (FieldKind, Failure)> {
        use FieldKind as F;
        let bad = |kind, detail: String| (kind, Failure::invalid(detail));
        for field in &self.fields {
            if field.input.len() > field.input.limit() {
                return Err(bad(
                    field.kind,
                    "The input is longer than this field permits.".into(),
                ));
            }
        }
        match &self.kind {
            FormKind::Description => {
                notes::validate(self.value(F::Text)).map_err(|e| bad(F::Text, e))?;
                Ok(Submission::Action(Action::Description {
                    text: self.value(F::Text).to_string(),
                }))
            }
            FormKind::PrivateNote(name) => {
                notes::validate(self.value(F::Text)).map_err(|e| bad(F::Text, e))?;
                Ok(Submission::Action(Action::PrivateNote {
                    name: name.clone(),
                    text: self.value(F::Text).to_string(),
                }))
            }
            FormKind::Project => {
                let town = self.value(F::Town).trim().to_string();
                if !crate::town::remote::valid_town(&town) {
                    return Err(bad(F::Town, "Use lowercase letters, digits and hyphens, such as mark50.".into()));
                }
                let name = self.optional(F::Name).ok_or_else(|| bad(F::Name, "Give the project a name.".into()))?;
                let what = self.optional(F::What).ok_or_else(|| bad(F::What, "Say what the project is for.".into()))?;
                let machines = self.list(F::Machines);
                if machines.is_empty() {
                    return Err(bad(F::Machines, "Name at least one machine.".into()));
                }
                for machine in &machines {
                    crate::world::validate_node_name(machine).map_err(|e| bad(F::Machines, e))?;
                }
                let mut places = Vec::new();
                for line in self.value(F::Places).lines().filter(|line| !line.trim().is_empty()) {
                    let parts: Vec<&str> = line.split('|').map(str::trim).collect();
                    let [what, machine, path] = parts[..] else {
                        return Err(bad(F::Places, format!("Write \"what it is | machine | path\": {line}")));
                    };
                    places.push(crate::town::project::Place { what: what.to_string(), location: location(machine, path).map_err(|e| bad(F::Places, e))? });
                }
                let canon = match self.optional(F::Canon) {
                    Some(text) => match text.split('|').map(str::trim).collect::<Vec<_>>()[..] {
                        [machine, path] => Some(location(machine, path).map_err(|e| bad(F::Canon, e))?),
                        _ => return Err(bad(F::Canon, "Write \"machine | path\".".into())),
                    },
                    None => None,
                };
                Ok(Submission::Action(Action::OpenProject {
                    town,
                    change: crate::town::ledger::CharterChange {
                        name: Some(name),
                        what: Some(what),
                        machines: Some(machines),
                        places: Some(places),
                        canon,
                        no_canon: false,
                    },
                }))
            }
            FormKind::Cursor => self
                .value(F::Cursor)
                .parse::<u64>()
                .map(Submission::Cursor)
                .map_err(|_| bad(F::Cursor, "Enter a nonnegative event cursor.".into())),
            FormKind::HubDirectory | FormKind::Export(_) => {
                let kind = if matches!(self.kind, FormKind::HubDirectory) {
                    F::Directory
                } else {
                    F::Path
                };
                let path = PathBuf::from(self.value(kind));
                if !path.is_absolute() {
                    return Err(bad(kind, "Enter an absolute path.".into()));
                }
                if let FormKind::Export(text) = &self.kind {
                    Ok(Submission::Action(Action::ExportInvite {
                        path,
                        text: text.clone(),
                    }))
                } else {
                    Ok(Submission::HubDirectory(path))
                }
            }
            FormKind::Invite => {
                let name = self.optional(F::Name);
                if let Some(name) = &name {
                    crate::world::validate_node_name(name).map_err(|e| bad(F::Name, e))?;
                }
                let hours = self
                    .value(F::Hours)
                    .parse::<u32>()
                    .ok()
                    .filter(|hours| (1..=8760).contains(hours))
                    .ok_or_else(|| bad(F::Hours, "Enter a number from 1 to 8760.".into()))?;
                let hubs = self.list(F::Hubs);
                for hub in &hubs {
                    crate::world::link::Endpoint::parse(hub).map_err(|e| bad(F::Hubs, e))?;
                }
                Ok(Submission::Action(Action::Invite { name, hours, hubs }))
            }
            FormKind::Grant(revision) => {
                let grant = Grant {
                    principal: self.value(F::Principal).trim().to_string(),
                    nodes: self.list(F::Targets),
                    verbs: self.list(F::Verbs),
                    expires: self.optional(F::Expires),
                };
                grant.validate_fields().map_err(|(field, detail)| {
                    bad(
                        match field {
                            GrantField::Principal => F::Principal,
                            GrantField::Targets => F::Targets,
                            GrantField::Verbs => F::Verbs,
                            GrantField::Expires => F::Expires,
                        },
                        detail,
                    )
                })?;
                Ok(Submission::Action(Action::Grant {
                    grant,
                    expected_revision: *revision,
                }))
            }
            FormKind::Create | FormKind::Join => {
                if matches!(self.kind, FormKind::Join) {
                    let invite = crate::world::invite::Invite::parse(self.value(F::Invite))
                        .map_err(|e| bad(F::Invite, e))?;
                    if invite.is_expired_at(time::OffsetDateTime::now_utc()) {
                        return Err(bad(
                            F::Invite,
                            "This invite has expired. Ask for a new invite.".into(),
                        ));
                    }
                }
                crate::world::validate_node_name(self.value(F::Name))
                    .map_err(|e| bad(F::Name, e))?;
                let tags = self.list(F::Tags);
                for tag in &tags {
                    crate::world::validate_node_name(tag).map_err(|e| bad(F::Tags, e))?;
                }
                let network = match self.value(F::Network) {
                    "auto" => NetworkMode::Auto,
                    "direct" => NetworkMode::Direct,
                    "system" => NetworkMode::System,
                    _ => return Err(bad(F::Network, "Choose auto, direct or system.".into())),
                };
                let name = self.value(F::Name).to_string();
                let interface = self.optional(F::Interface);
                let install_service = self.value(F::AutoService) == "yes";
                if matches!(self.kind, FormKind::Create) {
                    crate::world::link::Endpoint::parse(self.value(F::Hub))
                        .map_err(|e| bad(F::Hub, e))?;
                    if self.value(F::Bootstrap).is_empty() {
                        return Err(bad(
                            F::Bootstrap,
                            "Enter the hub's bootstrap password.".into(),
                        ));
                    }
                    Ok(Submission::Action(Action::Init {
                        hub: self.value(F::Hub).to_string(),
                        bootstrap: self.value(F::Bootstrap).to_string(),
                        name,
                        tags,
                        network,
                        interface,
                        install_service,
                    }))
                } else {
                    Ok(Submission::Action(Action::Enroll {
                        invite: self.value(F::Invite).to_string(),
                        name,
                        tags,
                        network,
                        interface,
                        install_service,
                    }))
                }
            }
        }
    }
}

/// A place on one of the World's machines, as a project's charter names it: the machine's name
/// and an absolute path in that machine's own spelling.
fn location(machine: &str, path: &str) -> Result<crate::town::project::Location, String> {
    crate::world::validate_node_name(machine)?;
    let bytes = path.as_bytes();
    let absolute = path.starts_with('/') || (bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\'));
    if !absolute {
        return Err(format!("{path} is not an absolute path."));
    }
    Ok(crate::town::project::Location { machine: machine.to_string(), path: path.to_string() })
}
