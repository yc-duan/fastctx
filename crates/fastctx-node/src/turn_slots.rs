//! How many turns one agent login runs at once (`world-usage.md`, 一个登录
//! 同时开几轮).
//!
//! A login shared by many meets the vendor's rate limits long before its
//! quota. So there is no limit at first; a turn the vendor refused for too
//! many requests halves it to half of what ran then, at most once a minute,
//! and each turn that ends well widens it again, by one each time as many
//! turns as it allows have ended, as congestion control does; an hour
//! without a refusal takes it away. A quota used up is no refusal of that
//! kind and tightens nothing. The owner may set a hard limit, with the
//! login's assistances. A turn past the limit waits, its words kept: the
//! owner's first, then the assisted accounts' one account after another,
//! and its conversation tells how many wait ahead. Only turns the World
//! starts wait; those someone starts in a host's own window count as
//! running all the same.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::ResidentId;
use fastctx_types::access::HubAccount;
use fastctx_types::residents::{About, Activity, Landed};
use fastctx_types::session::{TurnEnd, TurnHow};
use serde_json::Value;

use crate::Node;

/// One burst of refusals halves the limit once.
const HALVING_AT_MOST: Duration = Duration::from_secs(60);
/// How long without a refusal before the limit is taken away.
const CALM: Duration = Duration::from_secs(60 * 60);
/// How long a waiting turn keeps its place without asking again: the
/// retries of its words ask every few seconds, so one that stopped asking
/// has gone another way.
const ASKING: Duration = Duration::from_secs(30);
/// How long a turn let through counts as running before its host says it
/// began.
const STARTING: Duration = Duration::from_secs(120);

#[derive(Default)]
pub(crate) struct Slots(Mutex<HashMap<String, Login>>);

#[derive(Default)]
struct Login {
    /// The limit the vendor's refusals set, widened since; none is none.
    found: Option<f64>,
    halved: Option<Instant>,
    /// The turns waiting, in the order they came.
    waiting: Vec<Waiting>,
    /// Turns let through whose host has not said yet that they began.
    starting: Vec<(ResidentId, Instant)>,
    /// The assisted account whose turn went last.
    last: Option<HubAccount>,
}

struct Waiting {
    id: ResidentId,
    /// The account it works for when the login only assists it; none for
    /// the owner's turns.
    assisted: Option<HubAccount>,
    asked: Instant,
}

/// A turn waiting its place, with how many wait ahead of it.
pub(crate) struct Queued {
    pub ahead: u32,
}

impl Queued {
    pub(crate) fn landed(&self) -> Landed {
        Landed::Waiting {
            why: Some(format!("its agent login runs as many turns at once as it may; {} wait ahead of it", self.ahead)),
            answered: true,
        }
    }
}

/// Each agent's default login here, written out.
struct Defaults {
    codex: PathBuf,
    claude: Option<PathBuf>,
}

impl Defaults {
    fn of(node: &Node) -> Self {
        Self { codex: node.codex.default().config_home().to_path_buf(), claude: crate::usage::claude_default() }
    }

    /// The login a conversation runs in, by its agent and directory.
    fn login(&self, about: &About) -> Option<(String, PathBuf)> {
        let agent = about.host.agent.as_str();
        let dir = match (&about.home, agent) {
            (Some(home), _) => home.clone(),
            (None, "codex") => self.codex.clone(),
            (None, "claude") => self.claude.clone()?,
            _ => return None,
        };
        Some((format!("{agent}-{}", crate::homes::key(&dir)), dir))
    }
}

/// Whether `id`, about to start a turn in its login, may start it now; else
/// its place in the line, kept while it asks again.
pub(crate) fn admit(node: &Arc<Node>, id: &ResidentId, about: &About) -> Result<(), Queued> {
    let defaults = Defaults::of(node);
    let Some((key, dir)) = defaults.login(about) else { return Ok(()) };
    let hard = hard(node, about, &dir, &defaults);
    let assisted = assisted(node, about);
    let now = Instant::now();
    let mut slots = node.turn_slots.0.lock().expect("never poisoned");
    // Who runs is read under this lock: a turn let through counts as
    // starting until its host says it began, and as running from then, so
    // read before it, a turn could pass from one to the other unseen by both.
    let running = running(node, &key, &defaults, Some(id));
    let login = slots.entry(key).or_default();
    login.prune(now, &running);
    let Some(limit) = login.limit(hard) else {
        login.leave(id);
        return Ok(());
    };
    match login.waiting.iter_mut().find(|waiting| waiting.id == *id) {
        Some(waiting) => {
            waiting.asked = now;
            waiting.assisted = assisted;
        }
        None => login.waiting.push(Waiting { id: id.clone(), assisted, asked: now }),
    }
    let order = login.order();
    let place = order.iter().position(|waiting| waiting == id).expect("it waits in line");
    let free = (limit as usize).saturating_sub(running.len() + login.starting.len());
    if place >= free {
        return Err(Queued { ahead: place as u32 });
    }
    // Those ahead of it within the room go too, once they ask again.
    let others: Vec<ResidentId> = order.into_iter().take(free).filter(|waiting| waiting != id).collect();
    login.go(id, now);
    drop(slots);
    wake(node, others);
    Ok(())
}

/// How many turns wait ahead of `id`'s, while it waits for one.
pub(crate) fn ahead(node: &Node, id: &ResidentId) -> Option<u32> {
    let slots = node.turn_slots.0.lock().expect("never poisoned");
    slots.values().find_map(|login| {
        login
            .waiting
            .iter()
            .any(|waiting| waiting.id == *id)
            .then(|| login.order().iter().position(|waiting| waiting == id).expect("it waits in line") as u32)
    })
}

/// A turn of `about`'s login ended: the vendor's refusal tightens the
/// limit, a turn that ended well widens it, and the turns that now have room
/// are woken.
pub(crate) fn ended(node: &Arc<Node>, id: &ResidentId, about: &About, end: &TurnEnd) {
    let defaults = Defaults::of(node);
    let Some((key, dir)) = defaults.login(about) else { return };
    let refused = refused_for_rate(node, about, &dir, end);
    let hard = hard(node, about, &dir, &defaults);
    let now = Instant::now();
    let mut slots = node.turn_slots.0.lock().expect("never poisoned");
    if !refused && !slots.contains_key(&key) {
        return;
    }
    let running = running(node, &key, &defaults, None);
    let login = slots.entry(key.clone()).or_default();
    login.starting.retain(|(starting, _)| starting != id);
    if refused {
        if login.halved.is_none_or(|at| now.duration_since(at) >= HALVING_AT_MOST) {
            login.halve(running.len() + 1, now);
            node.log.line(format_args!(
                "the vendor refused a turn of {key} for too many requests: at most {} at once now",
                login.shown()
            ));
        }
    } else if end.how == TurnHow::Completed
        && let Some(found) = login.found
    {
        if login.halved.is_none_or(|at| now.duration_since(at) >= CALM) {
            login.found = None;
            node.log.line(format_args!("{key} had no refusal for an hour: no limit on its turns at once again"));
        } else {
            login.found = Some(found + 1.0 / found);
        }
    }
    login.prune(now, &running);
    let room = login
        .limit(hard)
        .map_or(usize::MAX, |limit| (limit as usize).saturating_sub(running.len() + login.starting.len()));
    let next: Vec<ResidentId> = login.order().into_iter().take(room).collect();
    if login.waiting.is_empty() && login.found.is_none() && login.starting.is_empty() {
        slots.remove(&key);
    }
    drop(slots);
    wake(node, next);
}

impl Login {
    /// The most turns at once: the lower of the one found and the owner's.
    fn limit(&self, hard: Option<u32>) -> Option<u32> {
        let found = self.found.map(|found| found.floor().max(1.0) as u32);
        match (found, hard) {
            (Some(found), Some(hard)) => Some(found.min(hard)),
            (found, hard) => found.or(hard),
        }
    }

    fn shown(&self) -> u32 {
        self.found.map_or(0, |found| found.floor().max(1.0) as u32)
    }

    /// Halves the limit to half of the `ran` turns running when the vendor
    /// refused, or of the limit when that was lower.
    fn halve(&mut self, ran: usize, now: Instant) {
        let before = self.found.map_or(ran as f64, |found| found.min(ran as f64));
        self.found = Some((before / 2.0).floor().max(1.0));
        self.halved = Some(now);
    }

    fn leave(&mut self, id: &ResidentId) {
        self.waiting.retain(|waiting| waiting.id != *id);
    }

    /// `id` goes: out of the line, counted as running until its host says
    /// so, and the next assisted account's turn is after its account's.
    fn go(&mut self, id: &ResidentId, now: Instant) {
        if let Some(at) = self.waiting.iter().position(|waiting| waiting.id == *id) {
            let gone = self.waiting.remove(at);
            if gone.assisted.is_some() {
                self.last = gone.assisted;
            }
        }
        self.starting.push((id.clone(), now));
    }

    /// Lets go of turns that stopped asking, and of turns let through that
    /// run now or never began.
    fn prune(&mut self, now: Instant, running: &[ResidentId]) {
        self.waiting.retain(|waiting| now.duration_since(waiting.asked) < ASKING);
        self.starting.retain(|(id, at)| !running.contains(id) && now.duration_since(*at) < STARTING);
    }

    /// The waiting turns in the order they go: the owner's as they came,
    /// then the assisted accounts' one account after another, beginning
    /// after the account that went last, each account's as they came.
    fn order(&self) -> Vec<ResidentId> {
        let mut order: Vec<ResidentId> = self
            .waiting
            .iter()
            .filter(|waiting| waiting.assisted.is_none())
            .map(|waiting| waiting.id.clone())
            .collect();
        let mut accounts: Vec<&HubAccount> =
            self.waiting.iter().filter_map(|waiting| waiting.assisted.as_ref()).collect();
        accounts.sort();
        accounts.dedup();
        if let Some(last) = &self.last {
            let after = accounts.iter().position(|account| *account > last).unwrap_or(0);
            accounts.rotate_left(after);
        }
        let mut lines: Vec<std::collections::VecDeque<ResidentId>> = accounts
            .iter()
            .map(|account| {
                self.waiting
                    .iter()
                    .filter(|waiting| waiting.assisted.as_ref() == Some(*account))
                    .map(|waiting| waiting.id.clone())
                    .collect()
            })
            .collect();
        while lines.iter().any(|line| !line.is_empty()) {
            for line in &mut lines {
                order.extend(line.pop_front());
            }
        }
        order
    }
}

/// The conversations of the login `key` running a turn now, but `except`.
fn running(node: &Node, key: &str, defaults: &Defaults, except: Option<&ResidentId>) -> Vec<ResidentId> {
    node.residents
        .everyone()
        .filter(|record| record.removed.is_none() && except != Some(&record.id))
        .filter(|record| record.now.as_ref().is_some_and(|now| now.turn.is_some() || now.activity == Activity::Busy))
        .filter(|record| defaults.login(&record.about).is_some_and(|(login, _)| login == key))
        .map(|record| record.id)
        .collect()
}

/// The account a conversation works for when its login only assists it.
fn assisted(node: &Node, about: &About) -> Option<HubAccount> {
    let fastctx_types::access::Acting::Accounts { accounts } = &about.acting else { return None };
    crate::assists::assisting(node, &about.acting).then(|| accounts.first().cloned()).flatten()
}

/// The owner's limit on the login `dir`: the lowest any live assistance of
/// it carries, as each hub keeps them.
fn hard(node: &Node, about: &About, dir: &std::path::Path, defaults: &Defaults) -> Option<u32> {
    let now = jiff::Timestamp::now();
    let default = match about.host.agent.as_str() {
        "codex" => Some(defaults.codex.as_path()),
        "claude" => defaults.claude.as_deref(),
        _ => None,
    };
    node.links()
        .iter()
        .filter_map(|link| {
            let view = link.view();
            let here = view.me().map(|member| member.name.clone())?;
            view.directory
                .assists
                .iter()
                .filter(|assist| assist.machine == here && assist.agent == about.host.agent && assist.live(now))
                .filter(|assist| {
                    let home = assist.home.as_deref().or(default);
                    home.is_some_and(|home| crate::homes::same_dir(home, dir))
                })
                .filter_map(|assist| assist.turns.map(u32::from))
                .min()
        })
        .min()
}

/// Whether the vendor refused the turn for too many requests: Codex says so
/// when its retries of a 429 ran out; Claude Code says `rate_limit` of that
/// and of a quota used up alike, so a refusal while the login's windows
/// were last read below full.
fn refused_for_rate(node: &Node, about: &About, dir: &std::path::Path, end: &TurnEnd) -> bool {
    if end.how == TurnHow::Completed {
        return false;
    }
    let Some(error) = &end.error else { return false };
    match about.host.agent.as_str() {
        "codex" => {
            error
                .get("codexErrorInfo")
                .or_else(|| error.pointer("/data/codexErrorInfo"))
                .and_then(|info| info.get("responseTooManyFailedAttempts"))
                .and_then(|attempts| attempts.get("httpStatusCode"))
                .and_then(Value::as_u64)
                == Some(429)
        }
        "claude" => {
            error.get("category").and_then(Value::as_str) == Some("rate_limit") && !used_up(node, "claude", dir)
        }
        _ => false,
    }
}

/// Whether a window of the login was full when last read, and has not
/// started again since.
fn used_up(node: &Node, agent: &str, dir: &std::path::Path) -> bool {
    let now = jiff::Timestamp::now();
    crate::usage::last_limits(node, agent, dir).is_some_and(|limits| {
        limits
            .windows
            .iter()
            .any(|window| window.used_tenths >= 1000 && window.resets_at.is_none_or(|resets| resets > now))
    })
}

/// Has the words of each of `ids` asked again now.
fn wake(node: &Arc<Node>, ids: Vec<ResidentId>) {
    for id in ids {
        crate::lifecycle::retry::kick(node, &id);
        crate::lifecycle::retry::changed(node, &id);
    }
}
