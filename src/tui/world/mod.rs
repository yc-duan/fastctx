//! World navigation, drafts, confirmations and asynchronous result ownership.

mod forms;
pub(super) mod i18n;
#[cfg(test)]
mod tests;
mod view;
mod widgets;
mod worker;

use crate::control::paths::ControlPaths;
use crate::world::client::NodeView;
use crate::world::control::{Action, Failure, ReadState, Receipt, Reply, ServiceAction, Snapshot};
use crate::world::messages::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use forms::{Form, FormEvent, FormKind, Submission};
use std::path::PathBuf;
use std::time::{Duration, Instant};
pub(super) use view::render;
use worker::Worker;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Page {
    #[default]
    Overview,
    Machines,
    Grants,
    Health,
    Events,
    Service,
    Hub,
}
impl Page {
    const ALL: [Self; 7] = [
        Self::Overview,
        Self::Machines,
        Self::Grants,
        Self::Health,
        Self::Events,
        Self::Service,
        Self::Hub,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|page| *page == self).unwrap_or(0)
    }
    fn title(self, m: &i18n::WorldMessages) -> &'static str {
        match self {
            Self::Overview => m.overview,
            Self::Machines => m.machines,
            Self::Grants => m.grants,
            Self::Health => m.health,
            Self::Events => m.events,
            Self::Service => m.service,
            Self::Hub => m.hub,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Screen {
    #[default]
    Page,
    Detail,
    Actions,
    Form,
    Confirm,
    Busy,
    Receipt,
    Failed,
    Discard,
    Help,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum MenuAction {
    Create,
    Join,
    Invite,
    Description,
    PrivateNote,
    Revoke,
    Leave,
    AddGrant,
    DeleteGrant,
    Install,
    Start,
    Restart,
    Stop,
    Uninstall,
    ChooseHub,
    HubRevoke,
    NextEvents,
    Cursor,
    Inspect,
    Refresh,
    Reconnect,
    OpenAgents,
    ServicePage,
    HubPage,
    Export,
}
impl MenuAction {
    fn label(self, m: &i18n::WorldMessages) -> &'static str {
        match self {
            Self::Create => m.create,
            Self::Join => m.join,
            Self::Invite => m.invite,
            Self::Description => m.edit_description,
            Self::PrivateNote => m.edit_note,
            Self::Revoke => m.revoke,
            Self::Leave => m.leave,
            Self::AddGrant => m.add_grant,
            Self::DeleteGrant => m.delete_grant,
            Self::Install => m.install,
            Self::Start => m.start,
            Self::Restart => m.restart,
            Self::Stop => m.stop,
            Self::Uninstall => m.uninstall,
            Self::ChooseHub => m.choose_hub,
            Self::HubRevoke => m.revoke,
            Self::NextEvents => m.next_events,
            Self::Cursor => m.event_cursor,
            Self::Inspect => m.inspect,
            Self::Refresh => m.refresh,
            Self::Reconnect => m.reconnect,
            Self::OpenAgents => m.open_agents,
            Self::ServicePage => m.service,
            Self::HubPage => m.hub,
            Self::Export => m.export,
        }
    }
}

pub(super) enum Navigation {
    Stay,
    Back,
    Agents,
}

pub(super) struct WorldApp {
    pub(super) page: Page,
    pub(super) screen: Screen,
    pub(super) snapshot: Option<Snapshot>,
    pub(super) load_error: Option<Failure>,
    form: Option<Form>,
    pub(super) preview: Option<Action>,
    pub(super) receipt: Option<Receipt>,
    pub(super) failure: Option<Failure>,
    pub(super) events: Vec<Event>,
    pub(super) event_cursor: u64,
    pub(super) event_latest: u64,
    pub(super) event_gap: bool,
    pub(super) events_loaded: bool,
    pub(super) event_error: Option<Failure>,
    pub(super) selected: [usize; 7],
    pub(super) menu_selected: usize,
    pub(super) scroll: usize,
    pub(super) max_scroll: usize,
    pub(super) viewport_rows: usize,
    pub(super) confirm_yes: bool,
    help_from: Screen,
    pub(super) hub_data: PathBuf,
    worker: Option<Worker>,
    active: Option<Action>,
    pending: Option<Action>,
    last_read: Option<Instant>,
    needs_input_drain: bool,
    stopping: bool,
}

impl WorldApp {
    pub(super) fn new(paths: &ControlPaths) -> Self {
        Self {
            page: Page::Overview,
            screen: Screen::Page,
            snapshot: None,
            load_error: None,
            form: None,
            preview: None,
            receipt: None,
            failure: None,
            events: Vec::new(),
            event_cursor: 0,
            event_latest: 0,
            event_gap: false,
            events_loaded: false,
            event_error: None,
            selected: [0; 7],
            menu_selected: 0,
            scroll: 0,
            max_scroll: 0,
            viewport_rows: 1,
            confirm_yes: false,
            help_from: Screen::Page,
            hub_data: paths.home.join(".fastctx-hub"),
            worker: None,
            active: None,
            pending: None,
            last_read: None,
            needs_input_drain: false,
            stopping: false,
        }
    }
    pub(super) fn enter(&mut self, paths: &ControlPaths) {
        if self.worker.is_none() {
            self.start(
                paths,
                Action::Snapshot {
                    hub_data: self.hub_data.clone(),
                },
            );
        }
    }
    pub(super) fn mutation_running(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| !worker.read_only)
            || self
                .pending
                .as_ref()
                .is_some_and(|action| !action.is_read())
    }
    pub(super) fn can_exit(&mut self) -> bool {
        if self.mutation_running() {
            self.screen = Screen::Busy;
            false
        } else {
            true
        }
    }

    pub(super) fn take_input_drain(&mut self) -> bool {
        std::mem::take(&mut self.needs_input_drain)
    }

    pub(super) fn cancel_reads_for_exit(&mut self) -> bool {
        self.stopping = true;
        if self.pending.as_ref().is_some_and(Action::is_read) {
            self.pending = None;
        }
        if let Some(worker) = &self.worker {
            worker.cancel_read();
            false
        } else {
            true
        }
    }
    pub(super) fn tick(&mut self, paths: &ControlPaths, visible: bool) {
        if let Some(reply) = self.worker.as_ref().and_then(Worker::poll) {
            self.worker = None;
            let action = self.active.take();
            if self.stopping && action.as_ref().is_some_and(Action::is_read) {
                return;
            }
            if self.pending.is_some() && action.as_ref().is_some_and(Action::is_read) {
                if let Some(pending) = self.pending.take() {
                    self.start(paths, pending);
                }
                return;
            }
            self.accept(reply, action.as_ref());
            if self.pending.is_some()
                && let Some(pending) = self.pending.take()
            {
                self.start(paths, pending);
                return;
            }
        }
        if visible
            && !self.stopping
            && self.worker.is_none()
            && !matches!(
                self.screen,
                Screen::Form | Screen::Confirm | Screen::Discard
            )
            && self
                .last_read
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
        {
            self.start(
                paths,
                Action::Snapshot {
                    hub_data: self.hub_data.clone(),
                },
            );
        }
    }
    fn start(&mut self, paths: &ControlPaths, action: Action) {
        if let Some(worker) = &self.worker {
            if worker.read_only {
                worker.cancel_read();
                self.pending = Some(action);
            } else {
                self.screen = Screen::Busy;
            }
            return;
        }
        let read = action.is_read();
        match Worker::spawn(paths, action.clone()) {
            Ok(worker) => {
                if matches!(action, Action::Snapshot { .. }) {
                    self.last_read = Some(Instant::now());
                }
                if !read {
                    self.screen = Screen::Busy;
                }
                self.active = Some(action);
                self.worker = Some(worker);
            }
            Err(error) => {
                self.failure = Some(error);
                self.screen = Screen::Failed;
            }
        }
    }
    fn accept(&mut self, reply: Reply, action: Option<&Action>) {
        match reply {
            Reply::Snapshot(snapshot) => {
                let selected_name = self.selected_node().map(|node| node.name.clone());
                self.hub_data = snapshot.hub_data.clone();
                self.snapshot = Some(*snapshot);
                self.load_error = None;
                if let Some(name) = selected_name
                    && let Some(index) = self.nodes().iter().position(|node| node.name == name)
                {
                    self.selected[Page::Machines.index()] = index;
                }
                self.clamp_selection();
            }
            Reply::Events(result) => {
                self.events_loaded = true;
                self.event_error = None;
                if let Some(first) = result.events.first()
                    && first.seq > self.event_cursor.saturating_add(1)
                {
                    self.event_gap = true;
                }
                self.event_latest = result.latest;
                for event in result.events {
                    if event.seq > self.event_cursor {
                        self.event_cursor = event.seq;
                        self.events.push(event);
                    }
                }
                self.clamp_selection();
            }
            Reply::Receipt(mut receipt) => {
                if matches!(action, Some(Action::ExportInvite { .. })) {
                    receipt.invite = self
                        .receipt
                        .as_ref()
                        .and_then(|previous| previous.invite.clone());
                }
                if receipt.enrolled || receipt.left {
                    self.events.clear();
                    self.event_cursor = 0;
                    self.events_loaded = false;
                    self.snapshot = None;
                }
                if receipt.left {
                    self.page = Page::Overview;
                }
                self.receipt = Some(receipt);
                self.failure = None;
                self.form = None;
                self.preview = None;
                self.screen = Screen::Receipt;
                self.scroll = 0;
                self.last_read = None;
                self.needs_input_drain = true;
            }
            Reply::Failed(error) => match action {
                Some(Action::Snapshot { .. }) => self.load_error = Some(error),
                Some(Action::Events { .. }) => self.event_error = Some(error),
                _ => {
                    self.failure = Some(error);
                    self.screen = Screen::Failed;
                    self.scroll = 0;
                    self.needs_input_drain = true;
                }
            },
        }
    }
    pub(super) fn nodes(&self) -> &[NodeView] {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.node.get())
            .and_then(|node| node.nodes.get())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    pub(super) fn selected_node(&self) -> Option<&NodeView> {
        self.nodes().get(self.selected[Page::Machines.index()])
    }
    fn enrolled(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| matches!(snapshot.config, ReadState::Ready(_)))
    }
    pub(super) fn actions(&self) -> Vec<MenuAction> {
        use MenuAction as A;
        if self.screen == Screen::Receipt
            && self
                .receipt
                .as_ref()
                .is_some_and(|receipt| receipt.invite.is_some())
        {
            return vec![A::Export];
        }
        if self.page == Page::Hub {
            let mut actions = vec![A::ChooseHub, A::Inspect];
            if self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.hub.get())
                .and_then(|hub| hub.members.get(self.selected[Page::Hub.index()]))
                .is_some_and(|member| member.state != "revoked")
            {
                actions.push(A::HubRevoke);
            }
            return actions;
        }
        if !self.enrolled() {
            return vec![A::Create, A::Join, A::HubPage, A::Inspect];
        }
        match self.page {
            Page::Overview => vec![
                A::Invite,
                A::Description,
                A::ServicePage,
                A::OpenAgents,
                A::Refresh,
                A::Leave,
            ],
            Page::Machines => {
                let mut list = vec![A::Invite, A::Refresh];
                if let Some(node) = self.selected_node() {
                    list.insert(0, A::PrivateNote);
                    if node.is_self {
                        list.insert(1, A::Description);
                    } else if node.state != "revoked" {
                        list.push(A::Revoke);
                    }
                }
                list
            }
            Page::Grants => {
                let mut list = vec![A::AddGrant, A::Refresh];
                if self.grants_len() > 0 {
                    list.push(A::DeleteGrant);
                }
                list
            }
            Page::Health => vec![A::Inspect, A::Refresh, A::Reconnect, A::ServicePage],
            Page::Events => vec![A::NextEvents, A::Cursor],
            Page::Service => vec![
                A::Start,
                A::Install,
                A::Restart,
                A::Stop,
                A::Uninstall,
                A::Inspect,
            ],
            Page::Hub => unreachable!(),
        }
    }
    fn grants_len(&self) -> usize {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.node.get())
            .map_or(0, |node| node.grants.grants.len())
    }
    fn count(&self) -> usize {
        if !self.enrolled() && self.page != Page::Hub {
            return self.actions().len();
        }
        match self.page {
            Page::Machines => self.nodes().len(),
            Page::Grants => self.grants_len(),
            Page::Events => self.events.len(),
            Page::Hub => self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.hub.get())
                .map_or(0, |hub| hub.members.len()),
            Page::Overview | Page::Service => self.actions().len(),
            Page::Health => 0,
        }
    }
    fn clamp_selection(&mut self) {
        self.selected[self.page.index()] =
            self.selected[self.page.index()].min(self.count().saturating_sub(1));
    }
    fn change_page(&mut self, next: bool, paths: &ControlPaths) {
        self.page = Page::ALL
            [(self.page.index() + if next { 1 } else { Page::ALL.len() - 1 }) % Page::ALL.len()];
        self.screen = Screen::Page;
        self.scroll = 0;
        self.clamp_selection();
        if self.page == Page::Events && !self.events_loaded && self.enrolled() {
            self.start(
                paths,
                Action::Events {
                    since: self.event_cursor,
                    limit: 100,
                },
            );
        }
    }
    pub(super) fn paste(&mut self, text: &str) {
        if self.screen == Screen::Form
            && let Some(form) = &mut self.form
        {
            form.paste(text);
        }
    }

    #[cfg(windows)]
    pub(super) fn reject_paste(&mut self, count: Option<usize>) {
        if self.screen == Screen::Form
            && let Some(form) = &mut self.form
        {
            if let Some(count) = count
                && let Some(field) = form.fields.get_mut(form.selected)
            {
                field.input.error = Some(crate::tui::text_input::InputError::TooLong {
                    limit: field.input.limit(),
                    count,
                });
            } else {
                form.error = Some(Failure::invalid(
                    "The terminal did not finish the paste. Nothing was inserted; paste the complete value again.",
                ));
            }
        }
    }
    fn open_form(&mut self, kind: FormKind, initial: &str) {
        self.form = Some(Form::new(kind, initial));
        self.screen = Screen::Form;
        self.scroll = 0;
        self.failure = None;
    }
    fn prepare(&mut self, paths: &ControlPaths, action: Action) {
        if matches!(
            action,
            Action::Description { .. }
                | Action::PrivateNote { .. }
                | Action::ExportInvite { .. }
                | Action::Refresh
                | Action::Reconnect
        ) {
            self.start(paths, action);
        } else {
            self.preview = Some(action);
            self.confirm_yes = false;
            self.screen = Screen::Confirm;
            self.scroll = 0;
        }
    }
    fn dispatch(&mut self, paths: &ControlPaths, choice: MenuAction) -> Navigation {
        if self.mutation_running() {
            self.screen = Screen::Busy;
            return Navigation::Stay;
        }
        use MenuAction as A;
        self.screen = Screen::Page;
        match choice {
            A::Create => self.open_form(FormKind::Create, ""),
            A::Join => self.open_form(FormKind::Join, ""),
            A::Invite => self.open_form(FormKind::Invite, ""),
            A::Description => {
                let description = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.description.get())
                    .cloned();
                if let Some(description) = description {
                    self.open_form(FormKind::Description, &description);
                } else {
                    self.failure = Some(Failure::new(
                        "The saved description could not be read. Check the local state and repair the reported file first.",
                    ));
                    self.screen = Screen::Failed;
                }
            }
            A::PrivateNote => {
                if let Some(node) = self.selected_node() {
                    let name = node.name.clone();
                    let note = node.private_note.clone().unwrap_or_default();
                    self.open_form(FormKind::PrivateNote(name), &note);
                }
            }
            A::Revoke => {
                if let Some(node) = self.selected_node() {
                    self.prepare(
                        paths,
                        Action::Revoke {
                            name: node.name.clone(),
                        },
                    );
                }
            }
            A::Leave => self.prepare(paths, Action::Leave),
            A::AddGrant => {
                if let Some(node) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.node.get())
                {
                    self.open_form(FormKind::Grant(node.grants.revision), "");
                } else {
                    self.failure = Some(Failure::new(
                        "The node service is unavailable. Start it and read the current access snapshot first.",
                    ));
                    self.screen = Screen::Failed;
                }
            }
            A::DeleteGrant => {
                if let Some(node) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.node.get())
                    && let Some(entry) = node.grants.grants.get(self.selected[Page::Grants.index()])
                {
                    self.prepare(
                        paths,
                        Action::DeleteGrant {
                            id: entry.id.clone(),
                            expected_revision: node.grants.revision,
                        },
                    );
                }
            }
            A::Install | A::Start | A::Restart | A::Stop | A::Uninstall => {
                let action = match choice {
                    A::Install => ServiceAction::Install,
                    A::Start => ServiceAction::Start,
                    A::Restart => ServiceAction::Restart,
                    A::Stop => ServiceAction::Stop,
                    _ => ServiceAction::Uninstall,
                };
                self.prepare(paths, Action::Service { action });
            }
            A::ChooseHub => {
                let initial = self.hub_data.to_string_lossy().into_owned();
                self.open_form(FormKind::HubDirectory, &initial);
            }
            A::HubRevoke => {
                if let Some(hub) = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.hub.get())
                    && let Some(member) = hub.members.get(self.selected[Page::Hub.index()])
                {
                    self.prepare(
                        paths,
                        Action::HubRevoke {
                            data: self.hub_data.clone(),
                            name: member.name.clone(),
                        },
                    );
                }
            }
            A::NextEvents => self.start(
                paths,
                Action::Events {
                    since: self.event_cursor,
                    limit: 100,
                },
            ),
            A::Cursor => self.open_form(FormKind::Cursor, &self.event_cursor.to_string()),
            A::Inspect => self.start(
                paths,
                Action::Snapshot {
                    hub_data: self.hub_data.clone(),
                },
            ),
            A::Refresh => self.prepare(paths, Action::Refresh),
            A::Reconnect => self.prepare(paths, Action::Reconnect),
            A::OpenAgents => return Navigation::Agents,
            A::ServicePage => {
                self.page = Page::Service;
                self.scroll = 0;
            }
            A::HubPage => {
                self.page = Page::Hub;
                self.scroll = 0;
            }
            A::Export => {
                if let Some(invite) = self
                    .receipt
                    .as_ref()
                    .and_then(|receipt| receipt.invite.clone())
                {
                    self.open_form(
                        FormKind::Export(invite),
                        &paths.home.join("world-invite.txt").to_string_lossy(),
                    );
                }
            }
        }
        Navigation::Stay
    }
    pub(super) fn handle_key(&mut self, key: KeyEvent, paths: &ControlPaths) -> Navigation {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return if self.can_exit() {
                Navigation::Back
            } else {
                Navigation::Stay
            };
        }
        if key.code == KeyCode::F(1)
            && matches!(self.screen, Screen::Page | Screen::Detail | Screen::Form)
        {
            self.help_from = self.screen;
            self.screen = Screen::Help;
            self.scroll = 0;
            return Navigation::Stay;
        }
        match self.screen {
            Screen::Help => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) => {
                    self.screen = self.help_from;
                    self.scroll = 0;
                }
                _ => self.scroll_key(key.code),
            },
            Screen::Form => {
                let event = self
                    .form
                    .as_mut()
                    .map(|form| form.key(key))
                    .unwrap_or(FormEvent::Back);
                match event {
                    FormEvent::Back => {
                        if self.form.as_ref().is_some_and(Form::dirty) {
                            self.screen = Screen::Discard;
                            self.confirm_yes = false;
                        } else {
                            self.form = None;
                            self.screen = Screen::Page;
                        }
                    }
                    FormEvent::Submit => {
                        if let Some(form) = &mut self.form {
                            match form.submit() {
                                Ok(Submission::Action(action)) => self.prepare(paths, action),
                                Ok(Submission::Cursor(cursor)) => {
                                    self.events.clear();
                                    self.event_cursor = cursor;
                                    self.event_latest = cursor;
                                    self.event_gap = false;
                                    self.events_loaded = false;
                                    self.form = None;
                                    self.screen = Screen::Page;
                                    self.selected[Page::Events.index()] = 0;
                                    self.start(
                                        paths,
                                        Action::Events {
                                            since: cursor,
                                            limit: 100,
                                        },
                                    );
                                }
                                Ok(Submission::HubDirectory(path)) => {
                                    self.hub_data = path;
                                    self.form = None;
                                    self.snapshot = None;
                                    self.screen = Screen::Page;
                                    self.start(
                                        paths,
                                        Action::Snapshot {
                                            hub_data: self.hub_data.clone(),
                                        },
                                    );
                                }
                                Err(_) => {}
                            }
                        }
                    }
                    FormEvent::None => {}
                }
            }
            Screen::Confirm | Screen::Discard => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    self.confirm_yes = !self.confirm_yes
                }
                KeyCode::Esc => {
                    self.screen = if self.form.is_some() {
                        Screen::Form
                    } else {
                        Screen::Page
                    }
                }
                KeyCode::Enter => {
                    if self.screen == Screen::Discard {
                        if self.confirm_yes {
                            self.form = None;
                            self.screen = Screen::Page;
                        } else {
                            self.screen = Screen::Form;
                        }
                    } else if self.confirm_yes {
                        if let Some(action) = self.preview.take() {
                            self.start(paths, action);
                        }
                    } else {
                        self.screen = if self.form.is_some() {
                            Screen::Form
                        } else {
                            Screen::Page
                        };
                    }
                }
                _ => self.scroll_key(key.code),
            },
            Screen::Busy => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                    self.screen = Screen::Page;
                }
            }
            Screen::Failed => match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    self.screen = if self.form.is_some() {
                        Screen::Form
                    } else {
                        Screen::Page
                    };
                    self.scroll = 0;
                }
                _ => self.scroll_key(key.code),
            },
            Screen::Receipt => match key.code {
                KeyCode::Char('e') | KeyCode::Char('a')
                    if self
                        .receipt
                        .as_ref()
                        .is_some_and(|receipt| receipt.invite.is_some()) =>
                {
                    return self.dispatch(paths, MenuAction::Export);
                }
                KeyCode::Esc | KeyCode::Enter => {
                    self.screen = Screen::Page;
                    self.scroll = 0;
                }
                _ => self.scroll_key(key.code),
            },
            Screen::Actions => {
                let actions = self.actions();
                match key.code {
                    KeyCode::Up => self.menu_selected = self.menu_selected.saturating_sub(1),
                    KeyCode::Down => {
                        self.menu_selected =
                            (self.menu_selected + 1).min(actions.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(action) = actions.get(self.menu_selected) {
                            return self.dispatch(paths, *action);
                        }
                    }
                    KeyCode::Esc => self.screen = Screen::Page,
                    _ => {}
                }
            }
            Screen::Detail => match key.code {
                KeyCode::Esc => {
                    self.screen = Screen::Page;
                    self.scroll = 0;
                }
                KeyCode::Char('a') => {
                    self.screen = Screen::Actions;
                    self.menu_selected = 0;
                }
                _ => self.scroll_key(key.code),
            },
            Screen::Page => match key.code {
                KeyCode::Esc => {
                    if self.mutation_running() {
                        self.screen = Screen::Busy;
                    } else {
                        return Navigation::Back;
                    }
                }
                KeyCode::Left => self.change_page(false, paths),
                KeyCode::Right | KeyCode::Tab => self.change_page(true, paths),
                KeyCode::BackTab => self.change_page(false, paths),
                KeyCode::PageUp if self.count() > 0 => {
                    self.selected[self.page.index()] =
                        self.selected[self.page.index()].saturating_sub(self.viewport_rows.max(1))
                }
                KeyCode::PageDown if self.count() > 0 => {
                    self.selected[self.page.index()] = self.selected[self.page.index()]
                        .saturating_add(self.viewport_rows.max(1))
                        .min(self.count().saturating_sub(1))
                }
                KeyCode::Home if self.count() > 0 => self.selected[self.page.index()] = 0,
                KeyCode::End if self.count() > 0 => {
                    self.selected[self.page.index()] = self.count().saturating_sub(1)
                }
                KeyCode::Char('a') => {
                    self.screen = Screen::Actions;
                    self.menu_selected = 0;
                }
                KeyCode::Char('r') => {
                    self.start(
                        paths,
                        if self.page == Page::Events {
                            Action::Events {
                                since: self.event_cursor,
                                limit: 100,
                            }
                        } else if self.enrolled() && self.page != Page::Hub {
                            Action::Refresh
                        } else {
                            Action::Snapshot {
                                hub_data: self.hub_data.clone(),
                            }
                        },
                    );
                }
                KeyCode::Up | KeyCode::Char('k') if self.count() > 0 => {
                    self.selected[self.page.index()] =
                        self.selected[self.page.index()].saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') if self.count() > 0 => {
                    self.selected[self.page.index()] =
                        (self.selected[self.page.index()] + 1).min(self.count().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if matches!(self.page, Page::Overview | Page::Service)
                        || (!self.enrolled() && self.page != Page::Hub)
                    {
                        if let Some(action) = self.actions().get(self.selected[self.page.index()]) {
                            return self.dispatch(paths, *action);
                        }
                    } else if self.count() > 0 {
                        self.screen = Screen::Detail;
                        self.scroll = 0;
                    } else {
                        self.screen = Screen::Actions;
                        self.menu_selected = 0;
                    }
                }
                _ => self.scroll_key(key.code),
            },
        }
        Navigation::Stay
    }
    fn scroll_key(&mut self, key: KeyCode) {
        self.scroll = match key {
            KeyCode::Up | KeyCode::Char('k') => self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll.saturating_add(1).min(self.max_scroll)
            }
            KeyCode::PageUp => self.scroll.saturating_sub(self.viewport_rows),
            KeyCode::PageDown => self
                .scroll
                .saturating_add(self.viewport_rows)
                .min(self.max_scroll),
            KeyCode::Home => 0,
            KeyCode::End => self.max_scroll,
            _ => self.scroll,
        };
    }
}
