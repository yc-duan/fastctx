//! World navigation, drafts, confirmations and asynchronous result ownership.

mod chrome;
mod forms;
pub(super) mod i18n;
mod model;
mod status_pages;
mod talk;
mod towns;
mod towns_view;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod towns_tests;
mod topology;
mod view;
mod widgets;
mod worker;

use crate::control::paths::ControlPaths;
use crate::world::client::NodeView;
use crate::world::control::{Action, Failure, ReadState, Receipt, Reply, ServiceAction, Snapshot};
use crate::world::messages::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use forms::{Form, FormEvent, FormKind, Submission};
use model::{LinkTone, service_facts};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};
pub(super) use view::render;
use worker::Worker;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Page {
    #[default]
    Overview,
    Towns,
    Machines,
    Grants,
    Connection,
    Events,
    Service,
    Hub,
}
impl Page {
    const ALL: [Self; 8] = [
        Self::Overview,
        Self::Towns,
        Self::Machines,
        Self::Grants,
        Self::Connection,
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
            Self::Towns => m.towns,
            Self::Machines => m.machines,
            Self::Grants => m.grants,
            Self::Connection => m.connection,
            Self::Events => m.events,
            Self::Service => m.service,
            Self::Hub => m.hub,
        }
    }
    /// Pages whose list is the machines of the World, so one selection follows the user
    /// between them.
    fn lists_machines(self) -> bool {
        matches!(self, Self::Overview | Self::Machines)
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
    /// A live conversation with one resident, or a new one being opened.
    Talk,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    Details,
    Refresh,
    Reconnect,
    OpenAgents,
    ServicePage,
    ConnectionPage,
    HubPage,
    Export,
    Talk,
    OpenResident,
    StopTurn,
    OpenProject,
}
impl MenuAction {
    fn from_action(action: &Action) -> Self {
        match action {
            Action::Snapshot { .. } => Self::Inspect,
            Action::Refresh => Self::Refresh,
            Action::Reconnect => Self::Reconnect,
            Action::Events { .. } => Self::NextEvents,
            Action::Init { .. } => Self::Create,
            Action::Enroll { .. } => Self::Join,
            Action::Description { .. } => Self::Description,
            Action::PrivateNote { .. } => Self::PrivateNote,
            Action::Invite { .. } => Self::Invite,
            Action::Revoke { .. } => Self::Revoke,
            Action::HubRevoke { .. } => Self::HubRevoke,
            Action::Grant { .. } => Self::AddGrant,
            Action::DeleteGrant { .. } => Self::DeleteGrant,
            Action::Leave => Self::Leave,
            Action::ExportInvite { .. } => Self::Export,
            Action::OpenProject { .. } => Self::OpenProject,
            Action::Service { action } => match action {
                ServiceAction::Install => Self::Install,
                ServiceAction::Start => Self::Start,
                ServiceAction::Restart => Self::Restart,
                ServiceAction::Stop => Self::Stop,
                ServiceAction::Uninstall => Self::Uninstall,
            },
        }
    }
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
            Self::Details => m.facts,
            Self::Refresh => m.refresh,
            Self::Reconnect => m.reconnect,
            Self::OpenAgents => m.open_agents,
            Self::ServicePage => m.service,
            Self::ConnectionPage => m.connection,
            Self::HubPage => m.hub,
            Self::Export => m.export,
            Self::Talk => m.talk,
            Self::OpenResident => m.open_resident,
            Self::StopTurn => m.stop_turn,
            Self::OpenProject => m.open_project,
        }
    }
    /// The single letter that runs this action from a page, shown next to its label.
    fn hotkey(self) -> Option<char> {
        match self {
            Self::Invite => Some('i'),
            Self::Description => Some('d'),
            Self::PrivateNote => Some('n'),
            Self::ServicePage => Some('s'),
            Self::Refresh => Some('r'),
            Self::Reconnect => Some('c'),
            Self::AddGrant | Self::Cursor | Self::ChooseHub => Some('e'),
            Self::NextEvents => Some('l'),
            Self::Talk => Some('t'),
            Self::OpenResident => Some('o'),
            Self::StopTurn => Some('x'),
            Self::OpenProject => Some('p'),
            _ => None,
        }
    }
}

pub(super) enum Navigation {
    Stay,
    Back,
    Agents,
}

/// Round-trip samples kept for the Connection chart.
const RTT_SAMPLES: usize = 120;
/// What an unenrolled machine can do, in the order the welcome page lists them.
pub(super) const WELCOME_ACTIONS: [MenuAction; 3] =
    [MenuAction::Create, MenuAction::Join, MenuAction::HubPage];

pub(super) struct WorldApp {
    pub(super) page: Page,
    pub(super) screen: Screen,
    pub(super) snapshot: Option<Snapshot>,
    pub(super) load_error: Option<Failure>,
    form: Option<Form>,
    pub(super) preview: Option<Action>,
    pub(super) receipt: Option<Receipt>,
    /// Keeps the operation label without retaining enrollment passwords or invite input.
    receipt_action: Option<MenuAction>,
    pub(super) failure: Option<Failure>,
    pub(super) events: Vec<Event>,
    pub(super) event_cursor: u64,
    pub(super) event_latest: u64,
    pub(super) event_gap: bool,
    pub(super) events_loaded: bool,
    pub(super) event_error: Option<Failure>,
    pub(super) selected: [usize; 8],
    towns: towns::Towns,
    pub(super) menu_selected: usize,
    pub(super) scroll: usize,
    pub(super) max_scroll: usize,
    pub(super) viewport_rows: usize,
    pub(super) confirm_yes: bool,
    help_from: Screen,
    pub(super) hub_data: PathBuf,
    /// Where the node writes its diagnostics; shown so a stuck service has a paper trail.
    pub(super) node_log: PathBuf,
    /// Hub round trips in arrival order, one per fresh status; the chart reads them left to right.
    pub(super) rtt_history: VecDeque<u32>,
    rtt_sampled_at: Option<String>,
    /// Frames drawn so far; only the busy indicator reads it.
    pub(super) frame: u64,
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
            receipt_action: None,
            failure: None,
            events: Vec::new(),
            event_cursor: 0,
            event_latest: 0,
            event_gap: false,
            events_loaded: false,
            event_error: None,
            selected: [0; 8],
            towns: towns::Towns::default(),
            menu_selected: 0,
            scroll: 0,
            max_scroll: 0,
            viewport_rows: 1,
            confirm_yes: false,
            help_from: Screen::Page,
            hub_data: paths.home.join(".fastctx-hub"),
            node_log: paths.fastctx_dir.join("world").join("node.log"),
            rtt_history: VecDeque::new(),
            rtt_sampled_at: None,
            frame: 0,
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
    pub(super) fn is_animating(&self) -> bool {
        self.screen == Screen::Busy
    }

    pub(super) fn tick(&mut self, paths: &ControlPaths, visible: bool) -> bool {
        let census_visible = visible && (self.page == Page::Towns || self.screen == Screen::Talk);
        let selected = self
            .towns
            .residents()
            .get(self.selected[Page::Towns.index()])
            .map(|facts| facts.about.id.clone());
        let mut changed = self.towns.tick(paths, census_visible, selected);
        if let Some(reply) = self.worker.as_ref().and_then(Worker::poll) {
            changed = true;
            self.worker = None;
            let action = self.active.take();
            if self.stopping && action.as_ref().is_some_and(Action::is_read) {
                return true;
            }
            if self.pending.is_some() && action.as_ref().is_some_and(Action::is_read) {
                if let Some(pending) = self.pending.take() {
                    self.start(paths, pending);
                }
                return true;
            }
            self.accept(reply, action.as_ref());
            if self.pending.is_some()
                && let Some(pending) = self.pending.take()
            {
                self.start(paths, pending);
                return true;
            }
        }
        if visible
            && !self.stopping
            && self.worker.is_none()
            // A refresh under an open menu can reorder the actions the user is aiming at, so the
            // modal screens all hold the snapshot still until they are dismissed.
            && !matches!(
                self.screen,
                Screen::Form | Screen::Confirm | Screen::Discard | Screen::Actions
            )
            && self
                .last_read
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
        {
            changed = true;
            self.start(
                paths,
                Action::Snapshot {
                    hub_data: self.hub_data.clone(),
                },
            );
        }
        changed
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
        let selected_action = (self.screen == Screen::Actions)
            .then(|| self.actions().get(self.menu_selected).copied())
            .flatten();
        match reply {
            Reply::Snapshot(snapshot) => {
                let selected_name = self.focused_node().map(|node| node.name.clone());
                self.hub_data = snapshot.hub_data.clone();
                self.sample_rtt(&snapshot);
                self.snapshot = Some(*snapshot);
                self.load_error = None;
                if let Some(name) = selected_name
                    && let Some(index) = self.nodes().iter().position(|node| node.name == name)
                {
                    self.selected[Page::Machines.index()] = index;
                    self.selected[Page::Overview.index()] = index;
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
                self.receipt_action = action.map(MenuAction::from_action);
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
                    self.rtt_history.clear();
                    self.rtt_sampled_at = None;
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
        if self.screen == Screen::Actions {
            let actions = self.actions();
            self.menu_selected = selected_action
                .and_then(|selected| actions.iter().position(|action| *action == selected))
                .or_else(|| {
                    actions
                        .iter()
                        .position(|action| *action == MenuAction::Inspect)
                })
                .unwrap_or(0);
        }
    }
    /// One sample per status the node actually wrote; repeated reads of the same status are
    /// not new measurements.
    fn sample_rtt(&mut self, snapshot: &Snapshot) {
        let Some(node) = snapshot.node.get() else {
            return;
        };
        if !crate::world::doctor::status_is_fresh(&node.status) {
            return;
        }
        if self.rtt_sampled_at.as_deref() == Some(node.status.written_at.as_str()) {
            return;
        }
        self.rtt_sampled_at = Some(node.status.written_at.clone());
        if let Some(rtt) = node.status.link.rtt_ms {
            if self.rtt_history.len() == RTT_SAMPLES {
                self.rtt_history.pop_front();
            }
            self.rtt_history.push_back(rtt);
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
    /// The machine the current page points at; Overview and Machines share the selection.
    pub(super) fn focused_node(&self) -> Option<&NodeView> {
        let page = if self.page.lists_machines() {
            self.page
        } else {
            Page::Machines
        };
        self.nodes().get(self.selected[page.index()])
    }
    pub(super) fn enrolled(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| matches!(snapshot.config, ReadState::Ready(_)))
    }
    fn welcome(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| matches!(snapshot.config, ReadState::Missing))
    }
    /// The page tabs exist once there is an enrollment to page through; the welcome page and
    /// the first load stand alone.
    pub(super) fn shows_tabs(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| !matches!(snapshot.config, ReadState::Missing))
    }
    /// The one action the current state asks for, if any: the first broken step of the
    /// service chain, from the outside in.
    pub(super) fn recommended(&self) -> Option<MenuAction> {
        if !self.enrolled() || self.page == Page::Hub {
            return None;
        }
        let snapshot = self.snapshot.as_ref()?;
        let facts = service_facts(snapshot);
        match (facts.running, facts.installed) {
            (Some(false), Some(true)) => return Some(MenuAction::Start),
            (Some(false), Some(false)) => return Some(MenuAction::Install),
            (Some(false), None) => return Some(MenuAction::Inspect),
            (None, _) => return Some(MenuAction::Inspect),
            (Some(true), _) => {}
        }
        match facts.link {
            Some(LinkTone::Connected) => {}
            Some(LinkTone::Stopped)
                if snapshot.node.get().is_some_and(|node| {
                    crate::world::doctor::link_findings(&node.status)
                        .iter()
                        .any(|finding| {
                            matches!(
                                finding,
                                crate::world::doctor::LinkFinding::Stopped {
                                    remedy: crate::world::doctor::StopRemedy::Revoked
                                        | crate::world::doctor::StopRemedy::ProtocolMismatch
                                        | crate::world::doctor::StopRemedy::HubIdentityMismatch,
                                    ..
                                }
                            )
                        })
                }) =>
            {
                return Some(if self.page == Page::Connection {
                    MenuAction::Details
                } else {
                    MenuAction::ConnectionPage
                });
            }
            Some(_) => return Some(MenuAction::Reconnect),
            None => return Some(MenuAction::Inspect),
        }
        if facts.hosting == Some(false) {
            return Some(MenuAction::Restart);
        }
        if snapshot
            .node
            .get()
            .is_some_and(|node| node.status.grant_stale)
        {
            return Some(MenuAction::Refresh);
        }
        if facts.installed == Some(false) {
            return Some(MenuAction::Install);
        }
        None
    }
    /// The action bound to Enter on a page. Lists otherwise open the selected object's details.
    /// Healthy services default to a read, so entering a page can never suggest a needless restart.
    pub(super) fn primary_action(&self) -> Option<MenuAction> {
        use MenuAction as A;
        if self.page == Page::Towns {
            return Some(if self.count() > 0 { A::Talk } else { A::OpenResident });
        }
        if self.page == Page::Hub {
            return (self.count() == 0).then_some(
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| matches!(snapshot.hub, ReadState::Missing))
                {
                    A::ChooseHub
                } else {
                    A::Inspect
                },
            );
        }
        if self.welcome() {
            return Some(
                WELCOME_ACTIONS[self.selected[self.page.index()].min(WELCOME_ACTIONS.len() - 1)],
            );
        }
        if !self.enrolled() || self.load_error.is_some() {
            return Some(A::Inspect);
        }
        if matches!(self.page, Page::Overview | Page::Service | Page::Connection)
            && let Some(action) = self.recommended()
        {
            return Some(action);
        }
        if self.page == Page::Service || self.page == Page::Connection {
            return Some(A::Inspect);
        }
        if self.page == Page::Overview && self.nodes().len() == 1 && self.nodes()[0].is_self {
            return Some(A::Invite);
        }
        if self.count() > 0 {
            return None;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !matches!(snapshot.node, ReadState::Ready(_)))
        {
            return Some(A::ServicePage);
        }
        Some(match self.page {
            Page::Overview | Page::Machines => A::Refresh,
            Page::Grants => A::AddGrant,
            Page::Events => A::NextEvents,
            _ => A::Inspect,
        })
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
        if self.page == Page::Towns {
            let mut list = vec![A::OpenResident];
            if let Some(facts) = self.towns.residents().get(self.selected[Page::Towns.index()]) {
                list.insert(0, A::Talk);
                if facts.now.state == "busy" {
                    list.push(A::StopTurn);
                }
            }
            list.push(A::OpenProject);
            list.push(A::Details);
            return list;
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
        if self.welcome() {
            return WELCOME_ACTIONS
                .into_iter()
                .chain([A::HubPage, A::Inspect])
                .collect();
        }
        if !self.enrolled() {
            return vec![A::Inspect, A::HubPage];
        }
        if self.page != Page::Service
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| !matches!(snapshot.node, ReadState::Ready(_)))
        {
            let mut actions = vec![A::ServicePage, A::Inspect, A::Leave];
            if let Some(action) = self.recommended() {
                actions.retain(|candidate| *candidate != action);
                actions.insert(0, action);
            }
            return actions;
        }
        let recommended = self.recommended();
        let mut list = match self.page {
            Page::Overview => {
                let mut list = vec![A::Invite];
                if let Some(node) = self.focused_node() {
                    list.push(A::PrivateNote);
                    if node.is_self {
                        list.push(A::Description);
                    }
                }
                list.extend([
                    A::ServicePage,
                    A::ConnectionPage,
                    A::Refresh,
                    A::OpenAgents,
                    A::Leave,
                ]);
                list
            }
            Page::Machines => {
                let mut list = vec![A::Invite, A::Refresh];
                if let Some(node) = self.focused_node() {
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
            Page::Connection => {
                let mut actions = vec![A::Inspect, A::Refresh, A::ServicePage];
                if self.snapshot.as_ref().is_some_and(|snapshot| {
                    service_facts(snapshot).link != Some(LinkTone::Connected)
                }) {
                    actions.push(A::Reconnect);
                }
                if recommended == Some(A::Details) {
                    actions.push(A::Leave);
                }
                actions
            }
            Page::Events => vec![A::NextEvents, A::Cursor],
            Page::Service => self.service_actions(),
            Page::Hub | Page::Towns => unreachable!(),
        };
        if let Some(action) = recommended
            && matches!(self.page, Page::Overview | Page::Service | Page::Connection)
        {
            list.retain(|candidate| *candidate != action);
            list.insert(0, action);
        } else if matches!(self.page, Page::Service | Page::Connection) {
            list.retain(|action| *action != A::Inspect);
            list.insert(0, A::Inspect);
        }
        // Every page's full facts are one menu entry away, so F1 need not be advertised.
        if !list.contains(&A::Details) {
            list.push(A::Details);
        }
        list
    }
    /// Service actions that make sense in the current state; the recommended one leads.
    fn service_actions(&self) -> Vec<MenuAction> {
        use MenuAction as A;
        let Some(snapshot) = &self.snapshot else {
            return vec![A::Inspect];
        };
        let facts = service_facts(snapshot);
        let mut list = match (facts.running, facts.installed) {
            (Some(false), Some(false)) => vec![A::Install, A::Start],
            (Some(false), Some(true)) => vec![A::Start, A::Uninstall],
            (Some(false), None) => vec![],
            (Some(true), Some(false)) => vec![A::Install, A::Restart, A::Stop],
            (Some(true), Some(true)) => vec![A::Restart, A::Stop, A::Uninstall],
            (Some(true), None) => vec![A::Restart, A::Stop],
            (None, _) => vec![],
        };
        if facts.running == Some(true) && facts.link.is_some_and(|tone| tone != LinkTone::Connected)
        {
            list.insert(0, A::Reconnect);
        }
        list.push(A::Inspect);
        list
    }
    fn selected_resident(&self) -> Option<String> {
        self.towns
            .residents()
            .get(self.selected[Page::Towns.index()])
            .map(|facts| facts.about.id.clone())
    }
    fn grants_len(&self) -> usize {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.node.get())
            .map_or(0, |node| node.grants.grants.len())
    }
    fn count(&self) -> usize {
        if self.welcome() && !matches!(self.page, Page::Hub | Page::Towns) {
            return WELCOME_ACTIONS.len();
        }
        match self.page {
            Page::Overview | Page::Machines => self.nodes().len(),
            Page::Towns => self.towns.residents().len(),
            Page::Grants => self.grants_len(),
            Page::Events => self.events.len(),
            Page::Hub => self
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.hub.get())
                .map_or(0, |hub| hub.members.len()),
            Page::Service => 0,
            Page::Connection => 0,
        }
    }
    fn clamp_selection(&mut self) {
        self.selected[self.page.index()] =
            self.selected[self.page.index()].min(self.count().saturating_sub(1));
        self.menu_selected = self
            .menu_selected
            .min(self.actions().len().saturating_sub(1));
    }
    fn select(&mut self, index: usize) {
        self.selected[self.page.index()] = index;
        if self.page.lists_machines() {
            self.selected[Page::Overview.index()] = index;
            self.selected[Page::Machines.index()] = index;
        }
    }
    fn go_to(&mut self, page: Page, paths: &ControlPaths) {
        self.page = page;
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
    fn change_page(&mut self, next: bool, paths: &ControlPaths) {
        let page = Page::ALL
            [(self.page.index() + if next { 1 } else { Page::ALL.len() - 1 }) % Page::ALL.len()];
        self.go_to(page, paths);
    }
    pub(super) fn paste(&mut self, text: &str) {
        if self.screen == Screen::Talk {
            self.towns.paste(text);
        }
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
                && let Some(index) = form.focused_field()
                && let Some(field) = form.fields.get_mut(index)
            {
                field.input.error = Some(crate::tui::text_input::InputError::TooLong {
                    limit: field.input.limit(),
                    count,
                });
            } else {
                form.error = Some(Failure::invalid(
                    "The terminal did not finish the paste. Nothing was inserted; paste the complete value again.",
                ));
                form.error_hint = Some(forms::ErrorHint::IncompletePaste);
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
                if let Some(node) = self.focused_node() {
                    let name = node.name.clone();
                    let note = node.private_note.clone().unwrap_or_default();
                    self.open_form(FormKind::PrivateNote(name), &note);
                }
            }
            A::Revoke => {
                if let Some(node) = self.focused_node() {
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
            A::Details => {
                self.help_from = Screen::Page;
                self.screen = Screen::Help;
                self.scroll = 0;
            }
            A::Refresh => self.prepare(paths, Action::Refresh),
            A::Reconnect => self.prepare(paths, Action::Reconnect),
            A::OpenAgents => return Navigation::Agents,
            A::ServicePage => self.go_to(Page::Service, paths),
            A::ConnectionPage => self.go_to(Page::Connection, paths),
            A::HubPage => self.go_to(Page::Hub, paths),
            A::Talk => {
                if let Some(id) = self.selected_resident() {
                    self.towns.talk_to(&id);
                    self.screen = Screen::Talk;
                }
            }
            A::OpenResident => {
                self.towns.compose();
                self.screen = Screen::Talk;
            }
            A::OpenProject => {
                // The machines that answered the census are offered to start with.
                let machines = self.towns.census().map(|census| census.machines.join(", ")).unwrap_or_default();
                self.open_form(FormKind::Project, &machines);
            }
            A::StopTurn => {
                if let Some(id) = self.selected_resident() {
                    self.towns.stop(&id);
                }
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
        // On the chat screen and in forms `?` is text; F1 still opens the facts.
        if (key.code == KeyCode::F(1)
            || (key.code == KeyCode::Char('?')
                && !matches!(self.screen, Screen::Form | Screen::Talk)))
            && matches!(
                self.screen,
                Screen::Page | Screen::Detail | Screen::Form | Screen::Receipt | Screen::Talk
            )
        {
            self.help_from = self.screen;
            self.screen = Screen::Help;
            self.scroll = 0;
            return Navigation::Stay;
        }
        match self.screen {
            Screen::Talk => {
                if let towns::ChatKey::Leave = self.towns.chat_key(key) {
                    self.screen = Screen::Page;
                    self.scroll = 0;
                }
            }
            Screen::Help => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::F(1) | KeyCode::Char('?') => {
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
                self.menu_selected = self.menu_selected.min(actions.len().saturating_sub(1));
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.menu_selected = self.menu_selected.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.menu_selected =
                            (self.menu_selected + 1).min(actions.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(action) = actions.get(self.menu_selected) {
                            return self.dispatch(paths, *action);
                        }
                    }
                    KeyCode::Esc => self.screen = Screen::Page,
                    KeyCode::Char(letter) => {
                        if let Some(action) = actions
                            .iter()
                            .find(|action| action.hotkey() == Some(letter))
                        {
                            return self.dispatch(paths, *action);
                        }
                    }
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
                KeyCode::Char('n') if self.page.lists_machines() => {
                    return self.dispatch(paths, MenuAction::PrivateNote);
                }
                _ => self.scroll_key(key.code),
            },
            Screen::Page => match key.code {
                KeyCode::Enter | KeyCode::Char('a') if self.mutation_running() => {
                    self.screen = Screen::Busy
                }
                KeyCode::Esc => {
                    if self.mutation_running() {
                        self.screen = Screen::Busy;
                    } else if self.welcome() && self.page == Page::Hub {
                        self.go_to(Page::Overview, paths);
                    } else {
                        return Navigation::Back;
                    }
                }
                KeyCode::Left | KeyCode::BackTab if self.shows_tabs() => {
                    self.change_page(false, paths)
                }
                KeyCode::Right | KeyCode::Tab if self.shows_tabs() => self.change_page(true, paths),
                KeyCode::Char(digit @ '1'..='8') if self.shows_tabs() => {
                    let index = usize::from(digit as u8 - b'1');
                    self.go_to(Page::ALL[index], paths);
                }
                KeyCode::PageUp if self.count() > 0 => {
                    self.select(
                        self.selected[self.page.index()].saturating_sub(self.viewport_rows.max(1)),
                    );
                }
                KeyCode::PageDown if self.count() > 0 => {
                    self.select(
                        self.selected[self.page.index()]
                            .saturating_add(self.viewport_rows.max(1))
                            .min(self.count().saturating_sub(1)),
                    );
                }
                KeyCode::Home if self.count() > 0 => self.select(0),
                KeyCode::End if self.count() > 0 => self.select(self.count().saturating_sub(1)),
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
                    self.select(self.selected[self.page.index()].saturating_sub(1));
                }
                KeyCode::Down | KeyCode::Char('j') if self.count() > 0 => {
                    self.select(
                        (self.selected[self.page.index()] + 1).min(self.count().saturating_sub(1)),
                    );
                }
                KeyCode::Enter => {
                    if let Some(action) = self.primary_action() {
                        return self.dispatch(paths, action);
                    } else if self.count() > 0 {
                        self.screen = Screen::Detail;
                        self.scroll = 0;
                    } else {
                        self.screen = Screen::Actions;
                        self.menu_selected = 0;
                    }
                }
                KeyCode::Char(letter) => {
                    if let Some(action) = self
                        .actions()
                        .into_iter()
                        .find(|action| action.hotkey() == Some(letter))
                    {
                        return self.dispatch(paths, action);
                    }
                    self.scroll_key(key.code);
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
