//! Per-login usage, measured locally by the hosts and committed as complete ledger entries.
pub(crate) mod accounts;
mod allowance;
mod calls;
mod checkpoint;
mod checks;
pub(crate) mod copied;
pub(crate) use calls::calls;
mod claude;
mod codex;
mod health;
mod history;
mod hold;
mod ledger;
mod meter;
mod quarantine;
mod readings;
mod recent;
mod reckoning;
mod report;
mod resets;
mod tail;
pub(crate) mod terminal_record;
mod week_read;

use crate::Node;
pub(crate) use allowance::{after_reading, fable_line, used_up};
use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::agents::AgentLimits;
use fastctx_types::residents::About;
use fastctx_types::session::{Happened, SessionChange, SessionEvent, TurnEnd, TurnTokens};
use fastctx_types::usage::Tokens;
pub(crate) use health::health;
pub(crate) use hold::watch;
use jiff::Timestamp;
use ledger::{Book, Entry};
pub(crate) use readings::codex_reading;
use readings::last_reading;
pub(crate) use report::{Reader, report};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
const KEPT_DAYS: i64 = 371;
const WEEK_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Default)]
pub(crate) struct Usage {
    books: Mutex<HashMap<PathBuf, Arc<Mutex<Book>>>>,
    failures: Mutex<HashMap<PathBuf, String>>,
    /// One lock for each login's check of its holds, by its ledger's name.
    enforcing: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    stopping: Mutex<std::collections::HashSet<(ResidentId, String)>>,
    /// Each Claude Code login's ledger by when what its weeks came to is
    /// next due to be measured (`stir`).
    measuring: Mutex<HashMap<PathBuf, Timestamp>>,
    logins: Mutex<HashMap<ResidentId, PathBuf>>,
    legacy: Mutex<Option<HashMap<String, claude::Legacy>>>,
}

fn root(node: &Node) -> PathBuf {
    node.world.path().join("usage")
}
fn ledger(node: &Node, agent: &str, dir: &Path) -> PathBuf {
    root(node).join(format!("{agent}-{}", crate::homes::key(dir)))
}

/// Where a login's week, as its owner set it, is kept: its dollars, in
/// millionths, beside its ledger.
const WEEK: &str = "week";

/// A login's week in quota dollars as its owner set it; none is the World's
/// default. It is what they expect a week to hold: each week's own figure
/// stands within a small reach of it (`reckoning`).
pub(crate) fn week(node: &Node, agent: &str, dir: &Path) -> Option<fastctx_types::usage::Usd> {
    week_at(&ledger(node, agent, dir))
}

/// The same of the login whose ledger is at `path`.
fn week_at(path: &Path) -> Option<fastctx_types::usage::Usd> {
    let kept = fastctx_platform::disk::read(path.join(WEEK)).ok()?;
    String::from_utf8(kept).ok()?.trim().parse::<u64>().ok().filter(|week| *week > 0).map(fastctx_types::usage::Usd)
}

pub(crate) use reckoning::{Asked as ResetAsked, MOST as MOST_WEEK, WIDEST as WIDEST_SWING};

/// How far a week's figure may stand from what the login's owner expects,
/// in percent either way, as they set it; none is the World's default.
pub(crate) fn swing(node: &Node, agent: &str, dir: &Path) -> Option<u8> {
    reckoning::swing(&ledger(node, agent, dir))
}

/// Keeps what a login's week is expected to hold, or forgets it for the
/// World's default, and, where given, how far a week's figure may stand
/// from it: under the ledger's lock, as everything kept beside it. The week
/// is written whole or not at all; what was kept of the two when the other
/// failed is said.
pub(crate) fn set_expected(
    node: &Node,
    agent: &str,
    dir: &Path,
    week: Option<fastctx_types::usage::Usd>,
    swing: Option<u8>,
) -> io::Result<()> {
    let path = ledger(node, agent, dir);
    with_book(node, &path, |_| {
        if let Some(swing) = swing {
            reckoning::set_swing(&path, swing)?;
        }
        let kept = path.join(WEEK);
        let written = match week {
            Some(week) => fastctx_platform::disk::create_dir_all(&path).and_then(|()| {
                fastctx_platform::publish(&kept, week.0.to_string().as_bytes(), fastctx_platform::Permissions::Private)
            }),
            None => match fastctx_platform::disk::remove_file(&kept) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
        };
        written.map_err(|error| match swing {
            Some(_) => io::Error::new(error.kind(), format!("the reach was kept, the week was not: {error}")),
            None => error,
        })
    })
}

/// Resets what everyone has used of this week of a login, or takes such a
/// reset back (`reckoning::reset`). What it refuses is told apart from a
/// ledger that cannot be read.
pub(crate) fn reset_usage(
    node: &Node,
    agent: &str,
    dir: &Path,
    asked: ResetAsked<'_>,
) -> io::Result<Result<(), String>> {
    let path = ledger(node, agent, dir);
    with_book(node, &path, |book| match reckoning::reset(&path, book, agent, Timestamp::now(), asked) {
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(Err(error.to_string())),
        made => made.map(Ok),
    })
}

/// Measures the weeks before this one of `agent`'s login `dir` where this
/// week has not been measured (`reckoning::measure_due`): once a week, on a
/// thread that may block, never two at once for a login, and not looked at
/// again before it can be due.
pub(crate) fn stir(node: &Arc<Node>, agent: &str, dir: &Path) {
    if agent != "claude" {
        return;
    }
    let path = ledger(node, agent, dir);
    {
        let mut due = node.usage.measuring.lock().expect("never poisoned");
        if due.get(&path).is_some_and(|until| *until > Timestamp::now()) {
            return;
        }
        // Taken while it is measured.
        due.insert(path.clone(), Timestamp::MAX);
    }
    let (node, dir) = (node.clone(), dir.to_path_buf());
    fastctx_platform::task::spawn(async move {
        let (working, at) = (node.clone(), path.clone());
        let measured = fastctx_platform::task::blocking(move || {
            with_book(&working, &at, |book| reckoning::measure_due(&at, book, "claude", Timestamp::now()))
        })
        .await;
        // Looked at again within the hour however long nothing is due: the
        // login may be in another account by then, whose weeks are not
        // measured yet.
        let hour = Timestamp::now() + jiff::SignedDuration::from_hours(1);
        let until = match &measured {
            Ok((until, _)) => {
                until.map_or(Timestamp::now() + jiff::SignedDuration::from_mins(10), |until| until.min(hour))
            }
            Err(_) => hour,
        };
        node.usage.measuring.lock().expect("never poisoned").insert(path.clone(), until);
        match measured {
            // The week's figure may have moved: what holds those the login
            // assists is looked at again.
            Ok((_, true)) => after_reading(&node, "claude", &dir).await,
            Ok(_) => {}
            Err(error) => {
                node.log.line(format_args!("measuring what the weeks of {} came to: {error}", path.display()))
            }
        }
    });
}

fn with_book<T>(node: &Node, path: &Path, f: impl FnOnce(&mut Book) -> io::Result<T>) -> io::Result<T> {
    let book = node.usage.books.lock().expect("never poisoned").entry(path.to_path_buf()).or_default().clone();
    let mut book = book.lock().expect("never poisoned");
    let result = book.load(path).and_then(|()| {
        // What a write is marked as made under (7965 P08).
        book.sharing = Some(crate::timeline::born_shared::now(node));
        f(&mut book)
    });
    let mut failures = node.usage.failures.lock().expect("never poisoned");
    match &result {
        Ok(_) => {
            failures.remove(path);
        }
        Err(error) => {
            failures.insert(path.to_path_buf(), error.to_string());
        }
    }
    result
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Week {
    pub start: Timestamp,
    pub end: Timestamp,
    pub window: bool,
}

pub(crate) async fn measurement(
    node: &Arc<Node>,
    id: &ResidentId,
    about: &About,
    event: &SessionEvent,
) -> io::Result<()> {
    let Some(dir) = login_of(node, id, about) else { return Ok(()) };
    let path = ledger(node, about.host.agent.as_str(), &dir);
    let changed = if about.host.agent.as_str() == "claude" {
        let Happened::Changed { change: SessionChange::Usage { usage }, .. } = &event.happened else { return Ok(()) };
        stir(node, "claude", &dir);
        match claude::measured(node, &path, id, about, usage, event.at) {
            Ok(changed) => changed,
            Err(error) => {
                after_reading(node, about.host.agent.as_str(), &dir).await;
                return Err(error);
            }
        }
    } else if about.host.agent.as_str() == "codex" {
        codex::measured(node, &path, id, about, event)?
    } else {
        false
    };
    if changed {
        after_reading(node, about.host.agent.as_str(), &dir).await;
    }
    Ok(())
}

/// Capture the accounting identity before the terminal commits. A resumed
/// conversation may use a different login by the time recovery runs.
pub(crate) fn terminal_record(
    node: &Node,
    id: &ResidentId,
    about: &About,
    turn: &str,
    end: &TurnEnd,
    at: Timestamp,
) -> Option<terminal_record::Record> {
    if about.host.agent.as_str() != "claude" {
        return None;
    }
    let facts = end.facts.as_deref()?;
    let tokens = tokens_of(facts.tokens.as_ref()?);
    if tokens.total() == 0 {
        return None;
    }
    let dir = login_of(node, id, about)?;
    Some(terminal_record::Record {
        ledger: format!("claude-{}", crate::homes::key(&dir)),
        session: about.session.clone(),
        at,
        who: who(about),
        with: with(about),
        model: facts.models.last().cloned().or_else(|| about.model.clone()).unwrap_or_default(),
        tokens,
        key: format!("terminal:{id}:{turn}"),
    })
}
pub(crate) fn record_terminal(
    node: &Node,
    directory: &fastctx_types::layout::ConversationDir,
    turn: &str,
    record: terminal_record::Record,
) -> io::Result<()> {
    let completed = crate::records::read(directory, |rows| {
        let kept: (String, String, bool) = rows.connection().query_row(
            "SELECT ledger,identity,complete FROM terminal_usage WHERE conversation=?1 AND turn=?2",
            (rows.conversation(), turn),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if kept.0 != record.ledger || kept.1 != record.key {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Corrupt,
                "terminal accounting identity changed",
            ));
        }
        Ok(kept.2)
    })?;
    if completed {
        return Ok(());
    }
    if !record.ledger.starts_with("claude-")
        || record.ledger.contains('/')
        || record.ledger.contains(char::from(92))
        || Path::new(&record.ledger).components().count() != 1
    {
        return Err(io::Error::other("invalid terminal usage ledger"));
    }
    let path = root(node).join(&record.ledger);
    with_book(node, &path, |book| {
        let identity = record.key.clone();
        apply_terminal(book, &path, &record)?;
        let (turn, ledger) = (turn.to_string(), record.ledger);
        crate::records::write_at(directory, fastctx_storage::Priority::Normal, 256, move |rows| {
            if rows.connection().execute(
                "UPDATE terminal_usage SET complete=1 WHERE conversation=?1 AND turn=?2 AND ledger=?3 AND identity=?4",
                (rows.conversation(), turn, ledger, identity),
            )? != 1
            {
                return Err(fastctx_storage::Error::new("terminal accounting receipt disappeared"));
            }
            Ok(())
        })
    })
}

fn apply_terminal(book: &mut Book, path: &Path, record: &terminal_record::Record) -> io::Result<()> {
    if book.keys.contains_key(&record.key) {
        return Ok(());
    }
    if record.session.as_ref().is_some_and(|session| book.modern.contains(session)) {
        return book.commit(path, Entry::Receipts { at: record.at, keys: vec![record.key.clone()] });
    }
    book.commit(
        path,
        Entry::Tokens {
            at: record.at,
            who: record.who.clone(),
            with: record.with.clone(),
            model: record.model.clone(),
            tokens: record.tokens,
            key: Some(record.key.clone()),
            call: None,
        },
    )
}

fn tokens_of(tokens: &TurnTokens) -> Tokens {
    match *tokens {
        TurnTokens::Claude { input_tokens, cache_read_input_tokens, cache_creation_input_tokens, output_tokens } => {
            Tokens {
                input: input_tokens,
                cache_read: cache_read_input_tokens,
                cache_write: cache_creation_input_tokens,
                output: output_tokens,
                reasoning: 0,
            }
        }
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
    }
}
/// Whom a conversation works for, as its usage counts it: the account it
/// was opened for, or none for the user of this machine's own sessions.
fn who(about: &About) -> Option<HubAccount> {
    match &about.acting {
        Acting::Here => None,
        Acting::Accounts { accounts } => accounts.first().cloned(),
    }
}

/// The other accounts a conversation works for alike, booked an even share
/// each with the first.
fn with(about: &About) -> Vec<HubAccount> {
    let Acting::Accounts { accounts } = &about.acting else { return Vec::new() };
    let mut rest: Vec<HubAccount> =
        accounts.iter().skip(1).filter(|account| Some(*account) != accounts.first()).cloned().collect();
    rest.sort();
    rest.dedup();
    rest
}

/// The login a conversation's usage counts under: the one it was started
/// in, and for a Claude session the user opened, the one whose `projects`
/// its transcript is in.
pub(crate) fn login_of(node: &Node, id: &ResidentId, about: &About) -> Option<PathBuf> {
    match about.host.agent.as_str() {
        "codex" => Some(about.home.clone().unwrap_or_else(|| node.codex.default().config_home().to_path_buf())),
        "claude" => {
            if let Some(home) = &about.home {
                return Some(home.clone());
            }
            if about.world_managed() {
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

/// Claude Code's default login: the one the user's environment names. Read
/// at most every `DEFAULT_READ_EVERY`: each turn's events ask for it, and
/// the user's environment is read from several files.
pub(crate) fn claude_default() -> Option<PathBuf> {
    static READ: std::sync::Mutex<Option<(std::time::Instant, Option<PathBuf>)>> = std::sync::Mutex::new(None);
    if let Some((at, default)) = READ.lock().expect("never poisoned").as_ref()
        && at.elapsed() < DEFAULT_READ_EVERY
    {
        return default.clone();
    }
    // Kept only when read: a reading that failed is tried again by the next
    // report, which would otherwise be taken as having no login to count.
    let env = crate::claude::environment().ok()?;
    let home = fastctx_platform::user_home().ok()?;
    let default = Some(crate::homes::claude_default(&env, &home));
    *READ.lock().expect("never poisoned") = Some((std::time::Instant::now(), default.clone()));
    default
}

/// How long the default Claude login, once read, is taken as it was.
const DEFAULT_READ_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

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
/// here do, and one who manages the login with its owner; one the login
/// only assists sees its own use alone (`world-agent-logins.md`, 用量).
fn sees_windows(node: &Node, about: &About) -> bool {
    let Some(account) = who(about) else { return true };
    let Some(link) = node.link_of(&account.hub) else { return false };
    let view = link.view();
    let Some(here) = view.me().map(|me| me.name.clone()) else { return false };
    view.logged_in(&here, &account.account)
        || view.rights(&account.account, &here).hands
        || crate::homes::assistance(node, &view, &account.account, &here, &about.host.agent, about.home.as_deref())
            .is_ok_and(|assist| assist.is_some_and(|assist| assist.manages()))
}
