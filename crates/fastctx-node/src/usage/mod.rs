//! What each agent's login here was used for (`world-agent-logins.md`, 用量),
//! in a ledger the node keeps for each login, a file a day under
//! `<world>/usage/<agent>-<login>/`. Claude Code's own cost ledger, which
//! the World's plugin hands over with every measurement, goes in as each
//! rise of a session's total; each turn's tokens go in at its end, for both
//! agents; each change of the login's usage windows goes in as a reading.
//! Nothing is asked of any vendor for it: these are figures the agents
//! already had.

mod allowance;
mod report;

use std::collections::HashMap;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::agents::{AgentLimits, LimitWindow};
use fastctx_types::residents::About;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, TurnEnd, TurnTokens};
use fastctx_types::usage::{Tokens, Usd};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Node;

pub(crate) use allowance::{after_reading, used_up};
use allowance::stop_at_allowance;
pub(crate) use report::report;

/// How long a day's file is kept: the calendar shows a year, and the week
/// it began in.
const KEPT_DAYS: i64 = 371;

/// One line of a login's ledger.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Entry {
    /// A rise of a session's cost total, by the agent's own ledger, with
    /// the model the session was on.
    Cost {
        at: Timestamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        who: Option<HubAccount>,
        model: String,
        usd: Usd,
    },
    /// A turn's tokens, at its end.
    Tokens {
        at: Timestamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        who: Option<HubAccount>,
        model: String,
        tokens: Tokens,
    },
    /// The login's usage windows, as a reading changed them.
    Reading { at: Timestamp, windows: Vec<LimitWindow> },
}

impl Entry {
    fn at(&self) -> Timestamp {
        match self {
            Entry::Cost { at, .. } | Entry::Tokens { at, .. } | Entry::Reading { at, .. } => *at,
        }
    }
}

/// What the ledger keeps in memory.
#[derive(Default)]
pub(crate) struct Usage {
    /// Each Claude session's cost total as last counted, loaded from
    /// `claude-costs.json` when first needed.
    costs: Mutex<Option<HashMap<String, Kept>>>,
    /// The last reading kept for each login, by its ledger, or that it has
    /// none.
    readings: Mutex<HashMap<String, Option<Reading>>>,
    /// The login each Claude conversation's sessions count under.
    logins: Mutex<HashMap<ResidentId, PathBuf>>,
    /// One write to the ledgers at a time.
    writing: Mutex<()>,
}

/// A reading of a login's windows: when it came, and the windows it had.
type Reading = (Timestamp, Vec<LimitWindow>);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Kept {
    usd: Usd,
    at: Timestamp,
}

/// The directory of all ledgers.
fn root(node: &Node) -> PathBuf {
    node.world.path().join("usage")
}

/// The ledger of `agent`'s login `dir`, named by the login's key, which two
/// spellings of one directory share.
fn ledger(node: &Node, agent: &str, dir: &Path) -> PathBuf {
    root(node).join(format!("{agent}-{}", crate::homes::key(dir)))
}

fn day_file(ledger: &Path, at: Timestamp) -> PathBuf {
    ledger.join(format!("{}.jsonl", at.strftime("%Y-%m-%d")))
}

fn append(node: &Node, ledger: &Path, entry: &Entry) -> io::Result<()> {
    let _writing = node.usage.writing.lock().expect("never poisoned");
    fastctx_platform::ensure_private_dir(ledger)?;
    let mut line = serde_json::to_vec(entry).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(day_file(ledger, entry.at()))?;
    file.write_all(&line)
}

/// The day files of `ledger` from the day of `since` on, oldest first.
fn days(ledger: &Path, since: Timestamp) -> io::Result<Vec<PathBuf>> {
    let first = since.strftime("%Y-%m-%d").to_string();
    let mut days: Vec<PathBuf> = match std::fs::read_dir(ledger) {
        Ok(read) => read
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "jsonl")
                    && path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| stem >= first.as_str())
            })
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    days.sort();
    Ok(days)
}

/// The entries of `ledger` from `since` on, oldest first. A line that does
/// not read is passed over: a later version may write kinds this one does
/// not know.
fn entries(ledger: &Path, since: Timestamp) -> io::Result<Vec<Entry>> {
    let mut found = Vec::new();
    for day in days(ledger, since)? {
        let text = std::fs::read_to_string(&day)?;
        for line in text.lines() {
            if let Ok(entry) = serde_json::from_str::<Entry>(line)
                && entry.at() >= since
            {
                found.push(entry);
            }
        }
    }
    Ok(found)
}

/// Whom a conversation works for, as its usage counts it: the account it
/// was opened for, or none for the user of this machine's own sessions.
fn who(about: &About) -> Option<HubAccount> {
    match &about.acting {
        Acting::Here => None,
        Acting::Accounts { accounts } => accounts.first().cloned(),
    }
}

/// The login a conversation's usage counts under: the one it was started
/// in, and for a Claude session the user opened, the one whose `projects`
/// its transcript is in.
fn login_of(node: &Node, id: &ResidentId, about: &About) -> Option<PathBuf> {
    match about.host.agent.as_str() {
        "codex" => Some(about.home.clone().unwrap_or_else(|| node.codex.default().config_home().to_path_buf())),
        "claude" => {
            if let Some(home) = &about.home {
                return Some(home.clone());
            }
            if about.world_started {
                return claude_default();
            }
            if let Some(known) = node.usage.logins.lock().expect("never poisoned").get(id) {
                return Some(known.clone());
            }
            let found = about
                .session
                .as_ref()
                .and_then(|session| {
                    let exchange = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
                    fastctx_hosts::claude::exchange::transcript(&exchange).ok().flatten()
                })
                .and_then(|transcript| crate::logins::claude_login_of(node, &transcript).ok())?;
            node.usage.logins.lock().expect("never poisoned").insert(id.clone(), found.clone());
            Some(found)
        }
        _ => None,
    }
}

/// Claude Code's default login: the one the user's environment names.
fn claude_default() -> Option<PathBuf> {
    let env = crate::claude::environment().ok()?;
    let home = fastctx_platform::user_home().ok()?;
    Some(crate::homes::claude_default(&env, &home))
}

/// A week of a login's use.
#[derive(Clone, Copy, Debug)]
pub(super) struct Week {
    pub start: Timestamp,
    pub end: Timestamp,
    /// Whether it is the login's own seven-day window; otherwise the seven
    /// days up to now.
    pub window: bool,
}

const WEEK_SECONDS: i64 = 7 * 24 * 60 * 60;

/// The week the login's own seven-day window is in, from the last reading
/// that had one: a week back from its reset, moved on by whole weeks once
/// that reset has passed, as the window itself begins again each week. The
/// seven days up to `now` where no reading had one.
pub(super) fn week(node: &Node, ledger: &Path, now: Timestamp) -> Week {
    let reset = last_reading(node, ledger)
        .into_iter()
        .flat_map(|(_, windows)| windows)
        .find(|window| window.minutes == Some(10_080))
        .and_then(|window| window.resets_at);
    let week = jiff::SignedDuration::from_secs(WEEK_SECONDS);
    match reset {
        Some(reset) => {
            let mut end = reset;
            if now >= end {
                let passed = now.duration_since(end).as_secs() / WEEK_SECONDS;
                end += jiff::SignedDuration::from_secs(WEEK_SECONDS * (passed + 1));
            }
            Week { start: end - week, end, window: true }
        }
        None => Week { start: now - week, end: now, window: false },
    }
}

/// How far back a login's last reading is looked for.
const READING_DAYS: i64 = 62;

/// The last reading kept in `ledger`, with when it came: read from its
/// files once, or that it has none, and known from then on as readings
/// come.
fn last_reading(node: &Node, ledger: &Path) -> Option<Reading> {
    let key = ledger.to_string_lossy().into_owned();
    if let Some(last) = node.usage.readings.lock().expect("never poisoned").get(&key) {
        return last.clone();
    }
    let since = Timestamp::now() - jiff::SignedDuration::from_hours(24 * READING_DAYS);
    let found = newest_reading(ledger, since).ok()?;
    // A reading kept while the files were read is newer than theirs.
    node.usage.readings.lock().expect("never poisoned").entry(key).or_insert(found).clone()
}

/// The newest reading in `ledger` from the day of `since` on, looked for
/// from the newest day back.
fn newest_reading(ledger: &Path, since: Timestamp) -> io::Result<Option<Reading>> {
    for day in days(ledger, since)?.into_iter().rev() {
        let text = std::fs::read_to_string(&day)?;
        let found = text.lines().rev().find_map(|line| match serde_json::from_str::<Entry>(line) {
            Ok(Entry::Reading { at, windows }) => Some((at, windows)),
            _ => None,
        });
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// The limits of `agent`'s login `dir` as they were last read, which its
/// ledger keeps past the sessions that read them and their conversations
/// (`world-agent-logins.md`, 额度).
pub(crate) fn last_limits(node: &Node, agent: &str, dir: &Path) -> Option<AgentLimits> {
    let (observed_at, windows) = last_reading(node, &ledger(node, agent, dir))?;
    Some(AgentLimits { observed_at, windows, credits: None })
}

/// The login's windows as last read, for the end of a turn of `about`;
/// none where the account it works for does not see them.
pub(crate) fn turn_limits(node: &Node, id: &ResidentId, about: &About) -> Option<AgentLimits> {
    if !sees_windows(node, about) {
        return None;
    }
    last_limits(node, about.host.agent.as_str(), &login_of(node, id, about)?)
}

/// Whether the account a conversation works for sees the windows of the
/// login it runs in: the user of this machine and an account that may work
/// here do; one the login only assists sees its own use alone
/// (`world-agent-logins.md`, 用量).
fn sees_windows(node: &Node, about: &About) -> bool {
    let Some(account) = who(about) else { return true };
    let Some(link) = node.link_of(&account.hub) else { return false };
    let view = link.view();
    let Some(here) = view.me().map(|me| me.name.clone()) else { return false };
    view.logged_in(&here, &account.account) || view.rights(&account.account, &here).hands
}

/// Counts a Claude session's measurement, as its plugin hands it over: a
/// rise of its cost and a change of the login's windows, either of which
/// can stop a turn at an allowance, the latter any turn on the login.
/// Measured again, the same total adds nothing. A failure is logged and
/// changes nothing else.
pub(crate) async fn measurement(node: &std::sync::Arc<Node>, id: &ResidentId, about: &About, event: &SessionEvent) {
    let Happened::Changed { change: SessionChange::Usage { usage }, .. } = &event.happened else { return };
    if about.host.agent.as_str() != "claude" {
        return;
    }
    match measured(node, id, about, usage, event.at) {
        Ok((_, Some(dir))) => after_reading(node, "claude", &dir).await,
        Ok((true, None)) => stop_at_allowance(node, id, about).await,
        Ok((false, None)) => {}
        Err(error) => node.log.line(format_args!("counting what {id} spent: {error}")),
    }
}

/// Counts a turn's tokens, once, as its end is first recorded.
pub(crate) fn turn_ended(node: &Node, id: &ResidentId, about: &About, end: &TurnEnd, at: Timestamp) {
    let Some(facts) = end.facts.as_deref() else { return };
    let Some(tokens) = &facts.tokens else { return };
    if let Err(error) = turn_tokens(node, id, about, tokens, facts.models.last().cloned(), at) {
        node.log.line(format_args!("counting the tokens of a turn of {id}: {error}"));
    }
}

/// A Claude session's measurement: its cost total, and the account's
/// windows. Whether its cost rose, and the login when its windows changed.
fn measured(
    node: &Node,
    id: &ResidentId,
    about: &About,
    usage: &Value,
    at: Timestamp,
) -> io::Result<(bool, Option<PathBuf>)> {
    let Some(dir) = login_of(node, id, about) else { return Ok((false, None)) };
    let ledger = ledger(node, "claude", &dir);
    let mut read = false;
    if let Some(windows) = usage.get("rateLimits").and_then(Value::as_array).map(|limits| claude_windows(limits))
        && !windows.is_empty()
    {
        read = reading(node, &ledger, windows, at)?;
    }
    let read = read.then_some(dir);
    let (Some(session), Some(total)) =
        (about.session.as_ref(), usage.pointer("/cost/usd").and_then(Value::as_f64).and_then(Usd::from_dollars))
    else {
        return Ok((false, read));
    };
    let rise = {
        let mut costs = node.usage.costs.lock().expect("never poisoned");
        let costs = loaded(node, &mut costs)?;
        let before = costs.get(session).map_or(Usd::ZERO, |kept| kept.usd);
        // A total below what was counted is a ledger begun again, as
        // Claude Code does for a session resumed after another in its
        // folder: it is counted from nothing.
        let rise = if total >= before { total.saturating_sub(before) } else { total };
        costs.insert(session.clone(), Kept { usd: total, at });
        keep_costs(node, costs)?;
        rise
    };
    if rise == Usd::ZERO {
        return Ok((false, read));
    }
    let model = about.model.clone().unwrap_or_default();
    append(node, &ledger, &Entry::Cost { at, who: who(about), model, usd: rise })?;
    Ok((true, read))
}

/// The windows in a Claude measurement's `rateLimits`, as the login's
/// slip draws them.
fn claude_windows(limits: &[Value]) -> Vec<LimitWindow> {
    limits
        .iter()
        .filter_map(|limit| {
            let kind = limit.get("kind")?.as_str()?.to_string();
            let percent = limit.get("percentUsed")?.as_f64().filter(|percent| *percent >= 0.0)?;
            Some(LimitWindow {
                minutes: match kind.as_str() {
                    "five_hour" => Some(300),
                    "seven_day" => Some(10_080),
                    _ => None,
                },
                used_tenths: (percent * 10.0).round() as u32,
                resets_at: limit.get("resetsAt").and_then(Value::as_str).and_then(|at| at.parse().ok()),
                name: kind,
            })
        })
        .collect()
}

/// Keeps a reading of a login's windows when it differs from the last.
/// Whether it did.
fn reading(node: &Node, ledger: &Path, windows: Vec<LimitWindow>, at: Timestamp) -> io::Result<bool> {
    let key = ledger.to_string_lossy().into_owned();
    {
        let mut readings = node.usage.readings.lock().expect("never poisoned");
        if readings.get(&key).is_some_and(|last| last.as_ref().is_some_and(|(_, last)| *last == windows)) {
            return Ok(false);
        }
        readings.insert(key, Some((at, windows.clone())));
    }
    append(node, ledger, &Entry::Reading { at, windows })?;
    Ok(true)
}

/// A reading of a Codex login's windows, as its app-server read or pushed
/// them. Whether they changed.
pub(crate) fn codex_reading(node: &Node, dir: &Path, windows: Vec<LimitWindow>, at: Timestamp) -> bool {
    if windows.is_empty() {
        return false;
    }
    reading(node, &ledger(node, "codex", dir), windows, at).unwrap_or_else(|error| {
        node.log.line(format_args!("keeping a reading of Codex's limits: {error}"));
        false
    })
}

/// A turn's tokens, by kind, under the model it ended on.
fn turn_tokens(
    node: &Node,
    id: &ResidentId,
    about: &About,
    tokens: &TurnTokens,
    model: Option<String>,
    at: Timestamp,
) -> io::Result<bool> {
    let Some(dir) = login_of(node, id, about) else { return Ok(false) };
    let tokens = match *tokens {
        TurnTokens::Claude { input_tokens, cache_read_input_tokens, cache_creation_input_tokens, output_tokens } => {
            Tokens {
                input: input_tokens,
                cache_read: cache_read_input_tokens,
                cache_write: cache_creation_input_tokens,
                output: output_tokens,
                reasoning: 0,
            }
        }
        // Codex counts cached input within its input, and reasoning
        // within its output.
        TurnTokens::Codex {
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            ..
        } => Tokens {
            input: input_tokens
                .saturating_sub(cached_input_tokens)
                .saturating_sub(cache_write_input_tokens.unwrap_or(0)),
            cache_read: cached_input_tokens,
            cache_write: cache_write_input_tokens.unwrap_or(0),
            output: output_tokens.saturating_sub(reasoning_output_tokens),
            reasoning: reasoning_output_tokens,
        },
    };
    if tokens.total() == 0 {
        return Ok(false);
    }
    let model = model.or_else(|| about.model.clone()).unwrap_or_default();
    let ledger = ledger(node, about.host.agent.as_str(), &dir);
    append(node, &ledger, &Entry::Tokens { at, who: who(about), model, tokens })?;
    Ok(true)
}

fn costs_file(node: &Node) -> PathBuf {
    root(node).join("claude-costs.json")
}

fn loaded<'a>(node: &Node, costs: &'a mut Option<HashMap<String, Kept>>) -> io::Result<&'a mut HashMap<String, Kept>> {
    if costs.is_none() {
        let read = match std::fs::read(costs_file(node)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error),
        };
        *costs = Some(read);
    }
    Ok(costs.as_mut().expect("just loaded"))
}

/// Keeps the totals, without sessions unheard of for a month: a session's
/// ledger is kept with it by Claude Code, which resumes it only from its
/// own record.
fn keep_costs(node: &Node, costs: &mut HashMap<String, Kept>) -> io::Result<()> {
    let month_ago = Timestamp::now() - jiff::SignedDuration::from_hours(24 * 31);
    costs.retain(|_, kept| kept.at >= month_ago);
    fastctx_platform::ensure_private_dir(&root(node))?;
    fastctx_platform::publish(
        &costs_file(node),
        &serde_json::to_vec(costs).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}

/// Deletes the days no longer kept, in every ledger.
pub(crate) fn collect(node: &Node) -> io::Result<()> {
    let oldest = (Timestamp::now() - jiff::SignedDuration::from_hours(24 * KEPT_DAYS)).strftime("%Y-%m-%d").to_string();
    let ledgers = match std::fs::read_dir(root(node)) {
        Ok(read) => read,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for ledger in ledgers.filter_map(|entry| entry.ok().map(|entry| entry.path())).filter(|path| path.is_dir()) {
        for day in std::fs::read_dir(&ledger)?.filter_map(|entry| entry.ok().map(|entry| entry.path())) {
            if day.extension().is_some_and(|extension| extension == "jsonl")
                && day.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| stem < oldest.as_str())
            {
                std::fs::remove_file(&day)?;
            }
        }
    }
    Ok(())
}
