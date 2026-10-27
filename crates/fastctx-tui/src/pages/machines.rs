//! This computer's hubs and machines (`world-network.md`): each hub it is
//! logged in to, how it is linked to it and with which accounts, each
//! machine there and what it is like; this computer's agents, a line for
//! each of their logins (`world-agent-logins.md`); logging in and out,
//! tokens for other devices, and who came into its accounts, with 「不是我」;
//! the networks it was on that were dissolved, said once.

use crossterm::event::{KeyCode, KeyEvent};
use fastctx_types::accounts::{Arrival, Dissolution, HubAccounts, Level};
use fastctx_types::agents::{AgentsReply, Integration};
use fastctx_types::logins::Drift;
use fastctx_types::world::{HubNodes, LinkState, NodesReply};
use fastctx_types::{AccountName, HubName, MachineName};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::logins::{self, Login};
use super::{List, Page, Sheet, quiet, sheet, title, wrong};
use crate::app::{Act, Ctx, Data, Done, Job};
use crate::design::Role;
use crate::pages::form;
use crate::paint::{self, Row};
use crate::sessions::{Report, Standing, Status};
use crate::text;

#[derive(Default)]
pub struct MachinesPage {
    nodes: Option<NodesReply>,
    /// Who came into this computer's accounts and was not shown yet.
    arrivals: Vec<Arrival>,
    /// The networks this computer was on that were dissolved, not yet seen.
    dissolved: Vec<Dissolution>,
    /// What this computer's accounts see of each hub, for their levels.
    accounts: Vec<HubAccounts>,
    failed: Option<String>,
    had_at: Option<Timestamp>,
    list: List,
    /// Whether the user's own sessions here have the World's tools.
    sessions: Option<Result<Status, String>>,
    /// This computer's agents, a line for each of their logins.
    agents: Option<Result<AgentsReply, String>>,
    /// What the latest apply or unapply came to, or that one is under way.
    outcome: Option<Outcome>,
    program: Option<Result<fastctx_types::renewal::Status, String>>,
}

/// What a row the keys can be on stands for.
enum Entry {
    Dissolved,
    Login(usize),
    Arrival(usize),
    Machine { hub: usize, machine: usize },
}

enum Outcome {
    Applying,
    Unapplying,
    Applied(Report),
    Unapplied(Report),
    NeedsRaise { current: i64, requested: u64 },
    Failed(String),
}

impl MachinesPage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(nodes: NodesReply) -> Self {
        Self { nodes: Some(nodes), had_at: Some(Timestamp::now()), ..Self::default() }
    }

    pub fn data(&mut self, data: Data) -> Act {
        match data {
            Data::Program(status) => self.program = Some(status),
            Data::Nodes(Ok(nodes)) => {
                self.nodes = Some(nodes);
                self.list.clamp(self.entries().len());
                self.failed = None;
                self.had_at = Some(Timestamp::now());
            }
            Data::Nodes(Err(why)) => self.failed = Some(why),
            Data::Accounts(accounts, dissolved) => {
                self.arrivals = accounts.iter().flat_map(|hub| hub.arrivals.iter().cloned()).collect();
                self.accounts = accounts;
                self.dissolved = dissolved;
                self.list.clamp(self.entries().len());
            }
            Data::Done { job: Job::Seen { .. } | Job::Remove { .. } | Job::SeenDissolved, result: Err(why) } => {
                self.failed = Some(why);
            }
            Data::Sessions(sessions) => self.sessions = Some(sessions),
            Data::Agents(agents) => {
                self.agents = Some(agents);
                self.list.clamp(self.entries().len());
            }
            Data::Done { job: Job::Apply { .. } | Job::Unapply | Job::ApplyHome { .. }, result } => {
                self.outcome = Some(match result {
                    Ok(Done::Applied(report)) => Outcome::Applied(report),
                    Ok(Done::Unapplied(report)) => Outcome::Unapplied(report),
                    Ok(Done::NeedsRaise { current, requested }) => Outcome::NeedsRaise { current, requested },
                    Ok(_) => return Act::Stay,
                    Err(why) => Outcome::Failed(why),
                });
            }
            _ => {}
        }
        Act::Stay
    }

    fn busy(&self) -> bool {
        matches!(self.outcome, Some(Outcome::Applying | Outcome::Unapplying))
    }

    fn any_in(&self) -> bool {
        let taken = |standing: Standing| matches!(standing, Standing::In | Standing::Old);
        matches!(&self.sessions, Some(Ok(status)) if taken(status.codex) || taken(status.claude))
    }

    fn any_installed(&self) -> bool {
        matches!(&self.sessions, Some(Ok(status)) if status.codex != Standing::Absent || status.claude != Standing::Absent)
    }

    /// The logins of this computer's agents, as drawn.
    fn logins_here(&self) -> Vec<Login> {
        match &self.agents {
            Some(Ok(reply)) => logins::logins(reply),
            _ => Vec::new(),
        }
    }

    /// The login the keys are on.
    fn selected_home(&self) -> Option<Login> {
        let Entry::Login(index) = self.entries().into_iter().nth(self.list.selected)? else { return None };
        self.logins_here().into_iter().nth(index)
    }

    /// The rows the keys can be on, in the order they are drawn.
    fn entries(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self.dissolved.iter().map(|_| Entry::Dissolved).collect();
        entries.extend((0..self.logins_here().len()).map(Entry::Login));
        entries.extend((0..self.arrivals.len()).map(Entry::Arrival));
        if let Some(nodes) = &self.nodes {
            for (hub, nodes) in nodes.hubs.iter().enumerate() {
                entries.extend(shown(nodes).into_iter().map(|machine| Entry::Machine { hub, machine }));
            }
        }
        entries
    }

    /// The accounts this computer is logged in to, hub by hub.
    fn logins(&self) -> Vec<(HubName, AccountName)> {
        let Some(nodes) = &self.nodes else { return Vec::new() };
        nodes.hubs.iter().flat_map(|hub| hub.logins.iter().map(|account| (hub.name.clone(), account.clone()))).collect()
    }

    /// The hub of the row the keys are on, and the first account this
    /// computer is logged in to there.
    fn selected_login(&self) -> Option<(HubName, AccountName)> {
        let nodes = self.nodes.as_ref()?;
        let hub = match self.entries().get(self.list.selected)? {
            Entry::Machine { hub, .. } => nodes.hubs.get(*hub)?,
            Entry::Arrival(index) => {
                let arrival = self.arrivals.get(*index)?;
                return Some((
                    nodes.hubs.iter().find(|hub| hub.hub == arrival.hub)?.name.clone(),
                    arrival.account.clone(),
                ));
            }
            Entry::Dissolved | Entry::Login(_) => nodes.hubs.first()?,
        };
        Some((hub.name.clone(), hub.logins.first()?.clone()))
    }

    /// The machine the keys are on, when this computer is logged in to the
    /// account it is registered under, and so may take it out.
    fn removable(&self) -> Option<(HubName, AccountName, MachineName)> {
        let nodes = self.nodes.as_ref()?;
        let entries = self.entries();
        let Entry::Machine { hub, machine } = entries.get(self.list.selected)? else { return None };
        let hub = nodes.hubs.get(*hub)?;
        let node = hub.machines.get(*machine)?;
        let owner = node.owner.as_ref().filter(|owner| hub.logins.contains(owner))?;
        Some((hub.name.clone(), owner.clone(), node.name.clone()))
    }

    /// Whether the keys are on a network said dissolved.
    fn on_dissolved(&self) -> bool {
        matches!(self.entries().get(self.list.selected), Some(Entry::Dissolved))
    }

    /// The arrival the keys are on, as the jobs name it.
    fn selected_arrival(&self) -> Option<(HubName, AccountName, MachineName)> {
        let entries = self.entries();
        let Entry::Arrival(index) = entries.get(self.list.selected)? else { return None };
        let arrival = self.arrivals.get(*index)?;
        let hub = self.nodes.as_ref()?.hubs.iter().find(|hub| hub.hub == arrival.hub)?;
        Some((hub.name.clone(), arrival.account.clone(), arrival.name.clone()))
    }

    pub fn key(&mut self, ctx: &Ctx, key: KeyEvent) -> Act {
        let count = self.entries().len();
        if self.list.key(&key, count) {
            return Act::Stay;
        }
        let first = self.nodes.as_ref().is_some_and(|nodes| nodes.hubs.is_empty());
        let logins = self.logins();
        match key.code {
            KeyCode::Char('l') => Act::Push(Page::Form(Box::new(form::FormPage::login(ctx, first)))),
            KeyCode::Char('t') if !logins.is_empty() => {
                let preset = self.selected_login();
                Act::Push(Page::Form(Box::new(form::FormPage::device(ctx, &logins, preset.as_ref()))))
            }
            KeyCode::Char('o') if !logins.is_empty() => {
                let preset = self.selected_login();
                Act::Push(Page::Form(Box::new(form::FormPage::logout(&logins, preset.as_ref()))))
            }
            KeyCode::Char('r') => match self.removable() {
                Some((hub, account, name)) => {
                    Act::Push(Page::Form(Box::new(form::FormPage::remove(hub, account, name))))
                }
                None => Act::Stay,
            },
            KeyCode::Char('y') if self.on_dissolved() => {
                self.dissolved.clear();
                self.list.clamp(self.entries().len());
                Act::Do(Job::SeenDissolved)
            }
            KeyCode::Char('y') => match self.selected_arrival() {
                Some((hub, account, name)) => Act::Do(Job::Seen { hub, account, name }),
                None => Act::Stay,
            },
            KeyCode::Char('n') => match self.selected_arrival() {
                Some((hub, account, name)) => {
                    let reason = ctx.words.not_me_reason().to_string();
                    Act::Do(Job::Remove { hub, account, name, reason, seen: true })
                }
                None => Act::Stay,
            },
            KeyCode::Char('u' | 'x' | 'U' | 't') if !self.busy() && self.selected_home().is_some() => {
                let login = self.selected_home().expect("a login is selected");
                let into = key.code != KeyCode::Char('x');
                let raise = key.code == KeyCode::Char('U');
                let take_back = key.code == KeyCode::Char('t');
                let changed = matches!(login.drift, Some(Drift::Changed { .. }));
                let can = match (into, login.standing) {
                    (true, Some(Integration::In)) if take_back => changed,
                    (true, _) if take_back => false,
                    (true, Some(Integration::Out | Integration::Old)) => true,
                    (true, Some(Integration::In)) => raise || login.drift.is_some(),
                    (false, Some(Integration::In | Integration::Old)) => true,
                    _ => false,
                };
                let Some(machine) = self.nodes.as_ref().map(|nodes| nodes.machine.clone()).filter(|_| can) else {
                    return Act::Stay;
                };
                self.outcome = Some(if into { Outcome::Applying } else { Outcome::Unapplying });
                Act::Do(Job::ApplyHome { host: login.host, home: login.home, into, raise, take_back, machine })
            }
            KeyCode::Char('u') if !self.busy() && self.any_installed() => {
                self.outcome = Some(Outcome::Applying);
                Act::Do(Job::Apply { raise: false })
            }
            KeyCode::Char('U') if matches!(self.outcome, Some(Outcome::NeedsRaise { .. })) => {
                self.outcome = Some(Outcome::Applying);
                Act::Do(Job::Apply { raise: true })
            }
            KeyCode::Char('x') if !self.busy() && self.any_in() => {
                self.outcome = Some(Outcome::Unapplying);
                Act::Do(Job::Unapply)
            }
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => Act::Back,
            _ => Act::Stay,
        }
    }

    /// The line on the user's own sessions, and what the latest apply or
    /// unapply came to.
    fn sessions_rows(&self, ctx: &Ctx, width: usize) -> Vec<Row> {
        let words = &ctx.words;
        let look = &ctx.look;
        let mut rows = Vec::new();
        let standing = match &self.sessions {
            None => quiet(ctx, words.asking()),
            Some(Err(why)) => wrong(ctx, text::first_line(why)),
            Some(Ok(status)) => {
                let role = |standing: Standing, verified: Option<bool>| match standing {
                    Standing::In if verified != Some(true) => Role::Caution,
                    Standing::In => Role::Alive,
                    Standing::Old => Role::Caution,
                    Standing::Out => Role::Muted,
                    Standing::Absent => Role::Faint,
                };
                let codex = status.codex_tools.as_ref().map(|check| check.result.is_ok());
                let claude = status.claude_tools.as_ref().map(|check| check.result.is_ok());
                Line::from(vec![
                    Span::styled(
                        words.tool_standing("Codex", status.codex, codex),
                        look.text(role(status.codex, codex)),
                    ),
                    Span::styled("  ·  ", look.text(Role::Faint)),
                    Span::styled(
                        words.tool_standing("Claude Code", status.claude, claude),
                        look.text(role(status.claude, claude)),
                    ),
                ])
            }
        };
        let label = Line::from(Span::styled(words.own_sessions(), look.text(Role::Muted)));
        if text::line_width(&label) + text::line_width(&standing) + 2 <= width {
            rows.push(Row::new(label).right(standing));
        } else {
            rows.push(Row::new(label));
            rows.extend(text::wrap(&standing.spans, width).into_iter().map(Row::new));
        }
        if let Some(Ok(status)) = &self.sessions {
            for check in [&status.codex_tools, &status.claude_tools].into_iter().flatten() {
                if let Err(reason) = &check.result {
                    for line in text::wrap_plain(&format!("{}: {reason}", check.host), look.text(Role::Caution), width)
                    {
                        rows.push(Row::new(line));
                    }
                }
            }
        }
        // What an old fastctx left here, and where it went: the user decides
        // whether to delete it (`world-upgrade.md`).
        if let Some(Ok(status)) = &self.sessions
            && let Some(legacy) = &status.legacy
            && (legacy.archived > 0 || legacy.tidied > 0 || legacy.waiting > 0)
        {
            let place = legacy.archives.last().unwrap_or(&legacy.record);
            let said = words.legacy_cleaned(legacy.tidied, legacy.archived, &shown_path(place), legacy.waiting);
            for line in text::wrap_plain(&said, look.text(Role::Faint), width) {
                rows.push(Row::new(line));
            }
        }
        let (said, role) = match &self.outcome {
            None => return rows,
            Some(Outcome::Applying) => (words.applying().to_string(), Role::Faint),
            Some(Outcome::Unapplying) => (words.unapplying().to_string(), Role::Faint),
            Some(Outcome::Applied(report)) if report.tools.iter().any(|check| check.result.is_err()) => {
                (words.tools_unverified().into(), Role::Caution)
            }
            Some(Outcome::Applied(report)) => {
                let claude = report.tools.iter().any(|check| check.host == "Claude Code");
                (words.applied(report.changed.len(), report.commands.len(), claude), Role::Muted)
            }
            Some(Outcome::Unapplied(report)) => {
                (words.unapplied(report.changed.len(), report.commands.len()), Role::Muted)
            }
            Some(Outcome::NeedsRaise { current, requested }) => {
                (words.needs_raise(*current, *requested), Role::Caution)
            }
            Some(Outcome::Failed(why)) => (why.clone(), Role::Wrong),
        };
        for line in text::wrap_plain(&said, look.text(role), width) {
            rows.push(Row::new(line));
        }
        if let Some(Outcome::Applied(report) | Outcome::Unapplied(report)) = &self.outcome {
            for message in &report.messages {
                for line in text::wrap_plain(message, look.text(Role::Caution), width) {
                    rows.push(Row::new(line));
                }
            }
            for check in &report.tools {
                let (said, role) = match &check.result {
                    Ok(()) => (format!("{}: {}", check.host, check.scope), Role::Faint),
                    Err(reason) => (format!("{}: {}: {reason}", check.host, check.scope), Role::Caution),
                };
                for line in text::wrap_plain(&said, look.text(role), width) {
                    rows.push(Row::new(line));
                }
            }
            for notice in &report.notices {
                for line in text::wrap_plain(
                    &words.left_alone(&notice.location, &notice.reason),
                    look.text(Role::Caution),
                    width,
                ) {
                    rows.push(Row::new(line));
                }
            }
            for leftover in &report.leftovers {
                for line in text::wrap_plain(leftover, look.text(Role::Faint), width) {
                    rows.push(Row::new(line));
                }
            }
        }
        rows
    }

    pub fn draw(&mut self, ctx: &Ctx, frame: &mut Frame, area: Rect) {
        let words = &ctx.words;
        let look = &ctx.look;
        let width = paint::column(area).width as usize;
        let mut rows = Vec::new();
        if let Some(program) = &self.program {
            rows.extend(super::program::rows(ctx, program, width));
        }
        let mut item = 0;
        if let Some(nodes) = &self.nodes {
            paint::open_section(&mut rows, paint::section(words.this_machine_link(), look));
            let mut first = vec![title(ctx, nodes.machine.to_string())];
            if let Some(facts) = &nodes.facts {
                first.push(Span::styled(
                    format!("   {} {}  ·  fastctx {}", facts.os, facts.arch, facts.version),
                    look.text(Role::Faint),
                ));
            }
            rows.push(Row::new(Line::from(first)));
            if nodes.hubs.is_empty() {
                for line in text::wrap_plain(&words.link_alone(), look.text(Role::Muted), width) {
                    rows.push(Row::new(line));
                }
            }
            for dissolution in &self.dissolved {
                let said = words.dissolved(dissolution.hub.as_str(), &crate::words::clock(dissolution.at, ctx.now));
                for line in text::wrap_plain(&said, look.text(Role::Caution), width) {
                    rows.push(Row::new(line).item(item));
                }
                item += 1;
            }
            rows.extend(self.sessions_rows(ctx, width));
            rows.push(Row::blank());
            paint::open_section(&mut rows, paint::section(words.agents_here(), look));
            match &self.agents {
                Some(Ok(reply)) => rows.extend(logins::rows(ctx, reply, &mut item, width)),
                Some(Err(why)) => rows.push(logins::asking(ctx, Some(why))),
                None => rows.push(logins::asking(ctx, None)),
            }
            rows.push(Row::blank());
            if !self.arrivals.is_empty() {
                paint::open_section(&mut rows, paint::section(words.arrivals(), look));
                for arrival in &self.arrivals {
                    let hub = nodes.hubs.iter().find(|hub| hub.hub == arrival.hub).map(|hub| hub.name.to_string());
                    let said = words.arrival(
                        arrival.name.as_str(),
                        arrival.device,
                        arrival.account.as_str(),
                        hub.as_deref().unwrap_or(arrival.hub.as_str()),
                        &crate::words::clock(arrival.noticed_at, ctx.now),
                    );
                    for line in text::wrap_plain(&said, look.text(Role::Caution), width) {
                        rows.push(Row::new(line).item(item));
                    }
                    item += 1;
                }
                rows.push(Row::blank());
            }
            for hub in &nodes.hubs {
                let seen = self.accounts.iter().find(|seen| seen.hub == hub.hub);
                hub_rows(ctx, &mut rows, &mut item, hub, seen, &nodes.machine, width);
            }
        }
        let note = match (&self.failed, self.had_at) {
            (Some(why), Some(at)) => Some(wrong(
                ctx,
                format!("{}  ·  {}", text::first_line(why), words.as_of(&crate::words::clock(at, ctx.now))),
            )),
            (Some(why), None) => Some(wrong(ctx, text::first_line(why))),
            (None, None) => Some(quiet(ctx, words.asking())),
            (None, Some(_)) => None,
        };
        let mut keys = vec![("l", words.key_login())];
        if !self.logins().is_empty() {
            keys.push(("t", words.key_device()));
            keys.push(("o", words.key_logout()));
        }
        if self.on_dissolved() {
            keys.push(("y", words.key_seen_dissolved()));
        } else if self.selected_arrival().is_some() {
            keys.push(("y", words.key_its_me()));
            keys.push(("n", words.key_not_me()));
        } else if self.removable().is_some() {
            keys.push(("r", words.key_remove()));
        }
        if let Some(login) = self.selected_home().filter(|_| !self.busy()) {
            match login.standing {
                Some(Integration::Out | Integration::Old) => keys.push(("u", words.key_apply_login())),
                Some(Integration::In) => {
                    match login.drift {
                        Some(Drift::Changed { .. }) => keys.push(("t", words.key_take_back())),
                        Some(Drift::Asks { .. }) => keys.push(("u", words.key_update_login())),
                        None => {}
                    }
                    keys.push(("x", words.key_unapply_login()));
                }
                _ => {}
            }
            if matches!(self.outcome, Some(Outcome::NeedsRaise { .. })) {
                keys.push(("U", words.key_raise()));
            }
        } else if matches!(self.outcome, Some(Outcome::NeedsRaise { .. })) {
            keys.push(("U", words.key_raise()));
        } else if self.any_installed() && !self.busy() {
            let old = matches!(&self.sessions, Some(Ok(status)) if status.codex == Standing::Old);
            keys.push(("u", if old { words.key_replace_old() } else { words.key_apply() }));
        }
        if self.any_in() && !self.busy() && self.selected_home().is_none() {
            keys.push(("x", words.key_unapply()));
        }
        keys.push(("Esc", words.key_back()));
        let page = Sheet {
            crumbs: vec![words.world().to_string(), words.machines().to_string()],
            right: Line::default(),
            rows,
            keys,
            note,
        };
        sheet(ctx, frame, area, &page, &mut self.list);
    }
}

/// One hub: how this computer is linked to it and with which accounts, at
/// what level, and each machine its accounts may use there.
fn hub_rows(
    ctx: &Ctx,
    rows: &mut Vec<Row>,
    item: &mut usize,
    hub: &HubNodes,
    seen: Option<&HubAccounts>,
    here: &MachineName,
    width: usize,
) {
    let words = &ctx.words;
    let look = &ctx.look;
    paint::open_section(
        rows,
        paint::section(&words.hub_section(hub.name.as_str()), look)
            .right(quiet(ctx, words.machine_count(hub.machines.len()))),
    );
    let (line, role) = match &hub.link {
        LinkState::Alone => (words.link_alone(), Role::Muted),
        LinkState::Connecting { since, last_error } => (
            words.link_connecting(
                &crate::words::clock(*since, ctx.now),
                last_error.as_deref().map(text::first_line).as_deref(),
            ),
            Role::Caution,
        ),
        LinkState::Online { since, hub, through, route, .. } => {
            (words.link_online(hub, through.as_deref(), route, &crate::words::clock(*since, ctx.now)), Role::Muted)
        }
        LinkState::Stopped { dissolved: true, .. } => (words.link_dissolved().to_string(), Role::Wrong),
        LinkState::Stopped { reason, .. } => (words.link_stopped(&text::first_line(reason)), Role::Wrong),
    };
    for line in text::wrap_plain(&line, look.text(role), width) {
        rows.push(Row::new(line));
    }
    let level = |account: &AccountName| {
        seen.and_then(|seen| seen.accounts.iter().find(|one| one.name == *account)).map(|one| one.level)
    };
    let logins = hub
        .logins
        .iter()
        .map(|login| match level(login) {
            Some(Level::Super) => format!("{login}{}", words.aside(words.level_super())),
            Some(Level::Visitor) => format!("{login}{}", words.aside(words.level_visitor())),
            _ => login.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut said = words.logins_line(&logins, hub.home.as_ref().map(|home| home.as_str()));
    // A member is told who the administrator is: that account may use every
    // machine of theirs; a visitor, that it only sees which there are
    // (`world-network.md`, 一个中枢，几个账号).
    let admin = seen.and_then(|seen| seen.accounts.iter().find(|one| one.level == Level::Super));
    if let Some(admin) = admin.filter(|admin| !hub.logins.contains(&admin.name)) {
        let visitors = hub.logins.iter().all(|login| level(login) == Some(Level::Visitor));
        said.push_str(&if visitors {
            words.admin_sees(admin.name.as_str())
        } else {
            words.admin_is(admin.name.as_str())
        });
    }
    for line in text::wrap_plain(&said, look.text(Role::Faint), width) {
        rows.push(Row::new(line));
    }
    rows.push(Row::blank());
    let mut group = None;
    for index in shown(hub) {
        let node = &hub.machines[index];
        // Each group's machines under its name.
        if let Some(name) = &node.group
            && group != Some((node.owner.as_ref(), name))
        {
            group = Some((node.owner.as_ref(), name));
            let owner = node.owner.as_ref().filter(|owner| !hub.logins.contains(owner)).map(|owner| owner.as_str());
            rows.push(Row::new(Line::from(Span::styled(words.group_head(owner, name), look.text(Role::Muted)))));
        }
        let (mark, since) = match node.online {
            Some(true) => (
                Span::styled("●", look.text(Role::Alive)),
                node.since.map(|since| words.online_since(&crate::words::clock(since, ctx.now))).unwrap_or_default(),
            ),
            Some(false) => (
                Span::styled("○", look.text(Role::Faint)),
                node.since
                    .map(|since| words.offline_since(&crate::words::clock(since, ctx.now)))
                    .unwrap_or_else(|| words.offline().to_string()),
            ),
            None => (Span::styled("·", look.text(Role::Muted)), String::new()),
        };
        let mut left = vec![mark, Span::raw("  "), title(ctx, node.written.clone())];
        if node.name == *here {
            left.push(Span::styled(format!("   {}", words.this_machine()), look.text(Role::Faint)));
        }
        rows.push(Row::new(Line::from(left)).right(quiet(ctx, since)).item(*item));
        let rights = node
            .rights
            .iter()
            .filter(|(_, rights)| rights.hands)
            .map(|(account, rights)| words.rights(account.as_str(), rights.hands))
            .collect::<Vec<_>>()
            .join("; ");
        let standing = words.owner_and_rights(node.owner.as_ref().map(|owner| owner.as_str()), &rights);
        rows.push(
            Row::new(Line::from(vec![Span::raw("   "), Span::styled(standing, look.text(Role::Muted))])).item(*item),
        );
        let second = match &node.facts {
            Ok(facts) => Some(Span::styled(
                format!("{} {}  ·  home {}  ·  fastctx {}", facts.os, facts.arch, facts.home, facts.version),
                look.text(Role::Faint),
            )),
            // The first row already says it is offline, and since when, or
            // that how it is is not known while its hub is not reached.
            Err(_) if node.online != Some(true) => None,
            Err(why) => Some(Span::styled(words.not_asked(&text::first_line(why)), look.text(Role::Caution))),
        };
        if let Some(second) = second {
            rows.push(Row::new(Line::from(vec![Span::raw("   "), second])).item(*item));
        }
        rows.push(Row::blank());
        *item += 1;
    }
}

/// The machines of a hub, by their places in it, in the order they are
/// drawn: those in no group first, then each group's by whose it is and its
/// name (`world-network.md`, 机器的分组).
fn shown(hub: &HubNodes) -> Vec<usize> {
    let mut order: Vec<usize> = (0..hub.machines.len()).collect();
    order.sort_by_key(|&at| {
        let node = &hub.machines[at];
        node.group.as_ref().map(|group| (node.owner.clone(), group.clone()))
    });
    order
}

/// A path as the TUI writes paths: with forward slashes on every system.
fn shown_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    if cfg!(windows) { text.replace('\\', "/") } else { text }
}
