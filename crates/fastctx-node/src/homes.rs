//! The agents' homes on this machine, one account apiece (`AgentHome`): how
//! two spellings of one directory are known as one, which of them a Claude
//! session's records belong to, and which the World may use. The World uses
//! only the logins in this machine's agent list, those its tools are put
//! into (`logins`); a session born in a login that has since been removed,
//! or whose directory is gone, is refused, never moved to another login.

use fastctx_types::access::Written;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::agents::{AgentHome, HomeChange};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};

use crate::Node;

/// Whether two directories are the same home, as this machine writes
/// paths: on Windows letters' case and either slash do not matter.
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    if normal(a) == normal(b) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => normal(&a) == normal(&b),
        _ => false,
    }
}

/// Whether `path` lies inside `dir`, as this machine writes paths.
pub(crate) fn within(dir: &Path, path: &Path) -> bool {
    location(path).starts_with(&format!("{}/", location(dir)))
}

fn normal(dir: &Path) -> String {
    let text = dir.to_string_lossy();
    let text = if cfg!(windows) { text.replace('\\', "/").to_lowercase() } else { text.into_owned() };
    let text = if cfg!(windows) {
        text.strip_prefix("//?/unc/")
            .map(|rest| format!("//{rest}"))
            .or_else(|| text.strip_prefix("//?/").map(str::to_string))
            .unwrap_or(text)
    } else {
        text
    };
    text.trim_end_matches('/').to_string()
}

fn location(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(path) => normal(&path),
        // Missing transcripts and removed homes retain their recorded path;
        // they never acquire another login's identity.
        Err(_) => normal(path),
    }
}

/// The most specific registered home containing a Claude transcript.
/// Removed homes still own their readings, which must not move to a parent.
pub(crate) fn limits_home<'a>(homes: &'a [AgentHome], transcript: &Path) -> Option<&'a AgentHome> {
    homes
        .iter()
        .filter(|home| within(&home.dir.join("projects"), transcript))
        .max_by_key(|home| location(&home.dir).len())
}

/// The legacy short name for a home's working directory. The Codex host
/// verifies its owner before reusing it, including actual hash collisions.
pub(crate) fn key(dir: &Path) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(normal(dir).as_bytes());
    digest[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Claude Code's default home: `CLAUDE_CONFIG_DIR` of the user's
/// environment, else `.claude` in the home directory.
pub(crate) fn claude_default(env: &[(OsString, OsString)], user_home: &Path) -> PathBuf {
    env.iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
        .filter(|(_, value)| !value.is_empty())
        .map(|(_, value)| PathBuf::from(value))
        .unwrap_or_else(|| user_home.join(".claude"))
}

/// The login of `agent` written `dir` (`None`: the default one) when it is
/// in this machine's agent list, as a session records it: `None` for the
/// default one, so that it follows the user's environment.
pub(crate) fn listed(node: &Node, agent: &str, dir: Option<&Path>) -> Result<Option<PathBuf>, Failure> {
    crate::logins::listed(node, agent, dir)
}

/// The assistance `account` has of `agent`'s login `home` on `machine`, a
/// login written as the node writes logins: none for the default one.
pub(crate) fn assistance(
    node: &Node,
    view: &fastctx_net::directory::Directory,
    account: &fastctx_types::AccountName,
    machine: &fastctx_types::MachineName,
    agent: &fastctx_types::AgentName,
    home: Option<&Path>,
) -> Result<Option<fastctx_net::directory::Assist>, Failure> {
    // The default login found once for all of them.
    let context = crate::logins::Context::of(node)?;
    let default = context.default_of(agent.as_str());
    let canonical = |home: Option<&Path>| home.filter(|home| !same_dir(home, default)).map(Path::to_path_buf);
    let home = canonical(home);
    Ok(view
        .assisting(account, machine)
        .find(|assist| assist.agent == *agent && canonical(assist.home.as_deref()) == home)
        .cloned())
}

pub(crate) fn may_use_login(
    node: &Node,
    view: &fastctx_net::directory::Directory,
    account: &fastctx_types::AccountName,
    machine: &fastctx_types::MachineName,
    agent: &fastctx_types::AgentName,
    home: Option<&Path>,
) -> Result<bool, Failure> {
    let Some(member) = view.member(machine) else { return Ok(false) };
    if member.managed || view.account(account).is_none_or(|account| account.locked) {
        return Ok(false);
    }
    Ok(view.rights(account, machine).hands
        || (!member.home.as_ref().is_some_and(|owner| view.is_visitor(owner))
            && assistance(node, view, account, machine, agent, home)?.is_some()))
}

/// Every existing session needs its login still listed and its directory present.
pub(crate) fn available(node: &Node, about: &fastctx_types::residents::About) -> Result<(), Failure> {
    listed(node, about.host.agent.as_str(), about.home.as_deref())?;
    Ok(())
}

/// What an older interface or node asks of the homes `machine` knows.
pub(crate) async fn call(node: &Arc<Node>, machine: Written, change: HomeChange) -> Result<(), Failure> {
    if node.is_here(&machine) {
        return self::change(node, change).await;
    }
    let call = RemoteCall::Homes { change };
    match node
        .call(&Acting::Here, &machine, Purpose::Agents, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::Set => Ok(()),
        _ => Err(Failure::new(format!("{machine} answered something other than its agents' homes"))),
    }
}

/// An older interface's change of the homes: adding one takes it up as a
/// candidate, its limits unread, as the workbench is not this interface's;
/// logins are no longer hidden, but removed from the list.
pub(crate) async fn change(node: &Arc<Node>, change: HomeChange) -> Result<(), Failure> {
    match change {
        HomeChange::Add { agent, dir } => crate::logins::take_up(node, agent, dir).await.map(|_| ()),
        HomeChange::Hide { .. } | HomeChange::Show { .. } => Err(Failure::not_run(
            "logins are no longer hidden or shown: a login is in the agent list once the World's tools are put into it, and removing it takes them out",
        )),
    }
}
