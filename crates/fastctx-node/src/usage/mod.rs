//! Per-login usage, measured locally by the hosts and committed as complete ledger entries.
pub(crate) mod accounts;
mod allowance;
mod calls;
pub(crate) use calls::calls;
mod claude;
mod codex;
mod compact;
mod health;
mod history;
mod hold;
mod ledger;
mod meter;
mod quarantine;
mod readings;
mod recent;
mod report;

use crate::Node;
pub(crate) use allowance::{after_reading, used_up};
pub(crate) use compact::collect;
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
pub(crate) use report::report;
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
    logins: Mutex<HashMap<ResidentId, PathBuf>>,
    legacy: Mutex<Option<HashMap<String, claude::Legacy>>>,
}

fn root(node: &Node) -> PathBuf {
    node.world.path().join("usage")
}
fn ledger(node: &Node, agent: &str, dir: &Path) -> PathBuf {
    root(node).join(format!("{agent}-{}", crate::homes::key(dir)))
}

fn with_book<T>(node: &Node, path: &Path, f: impl FnOnce(&mut Book) -> io::Result<T>) -> io::Result<T> {
    let book = node.usage.books.lock().expect("never poisoned").entry(path.to_path_buf()).or_default().clone();
    let mut book = book.lock().expect("never poisoned");
    let result = book.load(path).and_then(|()| f(&mut book));
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

/// Old plugins still report tokens only at the end of the main turn.
pub(crate) fn turn_ended(node: &Node, id: &ResidentId, about: &About, end: &TurnEnd, at: Timestamp) {
    if about.host.agent.as_str() != "claude" {
        return;
    }
    let Some(facts) = end.facts.as_deref() else { return };
    let Some(tokens) = &facts.tokens else { return };
    let Some(dir) = login_of(node, id, about) else { return };
    let path = ledger(node, "claude", &dir);
    let result = with_book(node, &path, |book| {
        if about.session.as_ref().is_some_and(|session| book.modern.contains(session)) {
            return Ok(());
        }
        let tokens = tokens_of(tokens);
        if tokens.total() == 0 {
            return Ok(());
        }
        book.commit(
            &path,
            Entry::Tokens {
                at,
                who: who(about),
                with: with(about),
                model: facts.models.last().cloned().or_else(|| about.model.clone()).unwrap_or_default(),
                tokens,
                key: Some(format!("legacy:{}:{at}", id)),
                call: None,
            },
        )
    });
    if let Err(error) = result {
        node.log.line(format_args!("counting the tokens of a turn of {id}: {error}"));
    }
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
fn login_of(node: &Node, id: &ResidentId, about: &About) -> Option<PathBuf> {
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

/// Claude Code's default login: the one the user's environment names.
pub(crate) fn claude_default() -> Option<PathBuf> {
    let env = crate::claude::environment().ok()?;
    let home = fastctx_platform::user_home().ok()?;
    Some(crate::homes::claude_default(&env, &home))
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
