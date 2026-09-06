//! World forms retain drafts across validation and operation failures.

use super::i18n::WorldMessages;
use crate::tui::text_input::{InputMode, TextInput};
use crate::world::control::{Action, Failure};
use crate::world::grant::Grant;
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
    Grant(u64),
    Cursor,
    HubDirectory,
    Export(String),
}

#[derive(Clone, Copy, Debug)]
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
}

impl FieldKind {
    pub(super) fn label(self, m: &WorldMessages, form: &FormKind) -> &'static str {
        match self {
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
        }
    }
    pub(super) fn help(self, m: &WorldMessages, form: &FormKind) -> &'static str {
        match self {
            Self::Name => m.name_help,
            Self::Hub | Self::Bootstrap => m.setup_help,
            Self::Invite => m.join_help,
            Self::Tags => m.tags_help,
            Self::Network => m.network_help,
            Self::Interface | Self::AutoService => m.optional_help,
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
        }
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

#[derive(Clone, Debug)]
pub(super) struct Form {
    pub(super) kind: FormKind,
    pub(super) fields: Vec<Field>,
    pub(super) selected: usize,
    pub(super) error: Option<Failure>,
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
            error: None,
        }
    }
    pub(super) fn title<'a>(&self, m: &'a WorldMessages) -> &'a str {
        match self.kind {
            FormKind::Create => m.create,
            FormKind::Join => m.join,
            FormKind::Invite => m.invite,
            FormKind::Description => m.edit_description,
            FormKind::PrivateNote(_) => m.edit_note,
            FormKind::Grant(_) => m.add_grant,
            FormKind::Cursor => m.event_cursor,
            FormKind::HubDirectory => m.choose_hub,
            FormKind::Export(_) => m.export,
        }
    }
    pub(super) fn dirty(&self) -> bool {
        self.fields
            .iter()
            .any(|field| field.original != field.input.text())
    }
    pub(super) fn paste(&mut self, value: &str) {
        if let Some(field) = self.fields.get_mut(self.selected) {
            if matches!(field.kind, FieldKind::Network | FieldKind::AutoService) {
                self.error = Some(Failure::invalid(
                    "Use Left/Right or Space to choose this option.",
                ));
                return;
            }
            field.input.insert(value);
            self.error = None;
        }
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
    pub(super) fn key(&mut self, key: KeyEvent) -> FormEvent {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            return FormEvent::Back;
        }
        if control && key.code == KeyCode::Char('s') {
            return FormEvent::Submit;
        }
        match key.code {
            KeyCode::BackTab => {
                self.selected = self.selected.checked_sub(1).unwrap_or(self.fields.len())
            }
            KeyCode::Tab => self.selected = (self.selected + 1) % (self.fields.len() + 1),
            KeyCode::Enter if self.selected == self.fields.len() => return FormEvent::Submit,
            KeyCode::Enter => {
                self.invite_name();
                self.selected += 1;
            }
            KeyCode::F(2) => {
                if let Some(field) = self.fields.get_mut(self.selected) {
                    field.input.revealed = !field.input.revealed;
                }
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if self.fields.get(self.selected).is_some_and(|field| {
                    matches!(field.kind, FieldKind::Network | FieldKind::AutoService)
                }) =>
            {
                let field = &mut self.fields[self.selected];
                let next = match (field.kind, field.input.text()) {
                    (FieldKind::Network, "auto") => "direct",
                    (FieldKind::Network, "direct") => "system",
                    (FieldKind::Network, _) => "auto",
                    (_, "yes") => "no",
                    _ => "yes",
                };
                field.input.replace(next.to_string());
            }
            _ => {
                if let Some(field) = self.fields.get_mut(self.selected)
                    && !matches!(field.kind, FieldKind::Network | FieldKind::AutoService)
                    && field.input.handle_key(key)
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
            .find(|field| std::mem::discriminant(&field.kind) == std::mem::discriminant(&kind))
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
        let result = self.validate();
        if let Err((kind, error)) = result {
            self.selected = self
                .fields
                .iter()
                .position(|field| {
                    std::mem::discriminant(&field.kind) == std::mem::discriminant(&kind)
                })
                .unwrap_or(0);
            self.error = Some(error.clone());
            Err(error)
        } else {
            Ok(result.unwrap_or_else(|_| unreachable!()))
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
                grant.validate().map_err(|e| {
                    bad(
                        if grant.principal.is_empty() {
                            F::Principal
                        } else if grant.nodes.is_empty() {
                            F::Targets
                        } else if grant.verbs.is_empty() {
                            F::Verbs
                        } else {
                            F::Expires
                        },
                        e,
                    )
                })?;
                Ok(Submission::Action(Action::Grant {
                    grant,
                    expected_revision: *revision,
                }))
            }
            FormKind::Create | FormKind::Join => {
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
                    let invite = crate::world::invite::Invite::parse(self.value(F::Invite))
                        .map_err(|e| bad(F::Invite, e))?;
                    if invite.is_expired_at(time::OffsetDateTime::now_utc()) {
                        return Err(bad(
                            F::Invite,
                            "This invite has expired. Ask for a new invite.".into(),
                        ));
                    }
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
