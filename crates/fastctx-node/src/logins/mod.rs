//! This machine's login records and their derived listing.
//!
//! Scans and connection changes are explicit operations. Sign-in file
//! changes are identified by a background pass; another pass derives the
//! listing, including program selection, connection state, models and usage.
//! Readers take the last persisted projection and current in-memory progress.
//! Integration actions still check their current apply records.

pub(crate) mod change;
pub(crate) mod folders;
pub(crate) mod heard;
pub(crate) mod identify;
pub(crate) mod kept;
pub(crate) mod limits;
pub(crate) mod models;
mod others;
mod program;
mod read_limits;
pub(crate) mod refresh;
pub(crate) mod remote;
pub(crate) mod scan;
mod search_progress;
mod sign_in;
pub(crate) mod sign_in_refresh;
mod snapshot;
pub(crate) mod submitted;
pub(crate) mod watch;
pub(crate) mod workbench;

use fastctx_types::access::Written;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use fastctx_hosts::inventory::walk;
use fastctx_install::apply::{self, Host, Standing};
use fastctx_types::AgentName;
use fastctx_types::agents::{AgentHome, AgentLimits, Fact, HomeFound, Integration};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{Found, LoginChange, MachineLogins};

use crate::Node;

/// The node's state of its logins beyond what is kept on disk.
#[derive(Default)]
pub(crate) struct Logins {
    /// Writing the kept records, one at a time.
    keeping: Mutex<()>,
    /// Whether a scan of the usual places is under way.
    scanning: AtomicBool,
    /// The searches of the chosen folders under way.
    searches: folders::Searches,
    /// The change running in each login, or the last one since the node
    /// started: its agent, directory and steps.
    changes: Mutex<Vec<(AgentName, PathBuf, LoginChange)>>,
    /// Changes of the list happen one at a time, as `fastctx apply` does.
    changing: crate::ranked::LoginChanges,
    /// The listed logins whose tools were not brought up to date by
    /// themselves, and why (`refresh`).
    drift: Mutex<Vec<(AgentName, PathBuf, fastctx_types::logins::Drift)>>,
    /// Whether bringing them up to date waits for the World program's
    /// replacement to be done (`refresh::replaced`).
    after_replacement: AtomicBool,
    /// Asking again the logins whose sign-in changed, one round at a time
    /// (`sign_in`).
    asking: crate::ranked::LoginAsking,
    /// The logins the workbench is open in, as last read (`workbench`).
    workbench: Mutex<workbench::Open>,
    /// Told when a login was connected or taken out, so that the files
    /// watched are those of the logins as they stand (`watch`).
    rewatch: Arc<tokio::sync::Notify>,
    /// Rebuilds the sign-in watch after the persisted login list changes.
    sign_in_watch: Arc<tokio::sync::Notify>,
    listing: snapshot::Listing,
}

/// The agents that keep logins.
pub(crate) const AGENTS: [&str; 2] = ["codex", "claude"];

fn host(agent: &str) -> Result<Host, Failure> {
    match agent {
        "codex" => Ok(Host::Codex),
        "claude" => Ok(Host::Claude),
        _ => Err(Failure::not_run(format!("{agent} keeps no logins the World knows of"))),
    }
}

/// What finding the logins needs: the user's environment and home, the
/// default login of each agent, and the places an apply touches.
pub(crate) struct Context {
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub user_home: PathBuf,
    pub codex_default: PathBuf,
    pub claude_default: PathBuf,
    pub places: apply::Places,
}

impl Context {
    pub(crate) fn of(node: &Node) -> Result<Self, Failure> {
        let env = crate::claude::environment()?;
        let user_home = fastctx_platform::user_home()
            .map_err(|error| Failure::new(format!("finding the home directory: {error}")))?;
        let claude_default = crate::homes::claude_default(&env, &user_home);
        let places = crate::agents::places(node, &env, &user_home, crate::claude::program());
        Ok(Self {
            codex_default: node.codex.default().config_home().to_path_buf(),
            claude_default,
            env,
            user_home,
            places,
        })
    }

    pub(crate) fn default_of(&self, agent: &str) -> &Path {
        if agent == "codex" { &self.codex_default } else { &self.claude_default }
    }
}

/// Known identities without credential probes or a system-directory scan.
pub(crate) fn identities(node: &Node, agent: &AgentName) -> Result<Vec<(PathBuf, bool)>, Failure> {
    host(agent.as_str())?;
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(error.to_string()))?;
    let default = context.default_of(agent.as_str());
    Ok(dirs(node, &context, &kept, agent.as_str())?
        .into_iter()
        .map(|(dir, _)| {
            let is_default = crate::homes::same_dir(&dir, default);
            (dir, is_default)
        })
        .collect())
}

/// The record of this machine's logins, without waiting for identification:
/// as last projected, which a read waits for only where none was ever made
/// (`snapshot::before_reading`).
pub(crate) async fn here(node: &Arc<Node>) -> Result<MachineLogins, Failure> {
    snapshot::before_reading(node).await;
    snapshot::read(node)
}

/// The record once what the call just changed is in it: the answer to a
/// call that changes a login shows the change.
pub(crate) async fn changed_here(node: &Arc<Node>) -> Result<MachineLogins, Failure> {
    snapshot::renew(node).await;
    snapshot::read(node)
}

/// The same projection in the shape older clients read.
pub(crate) async fn agents(node: &Arc<Node>) -> Result<fastctx_types::agents::AgentsReply, Failure> {
    snapshot::before_reading(node).await;
    snapshot::agents(node)
}

/// The usage of `agent`'s login `dir` was read anew: its row is projected
/// again.
pub(crate) fn usage_read(node: &Node, agent: &str, dir: &Path) {
    if AGENTS.contains(&agent) {
        snapshot::quota(node, agent, dir);
    }
}

/// Each login of `agent` the World knows here, with how it knows of it:
/// the default one, those the last scan found or the user took up, those
/// the user took up since, and those the World's tools are put into.
fn dirs(node: &Node, context: &Context, kept: &kept::Kept, agent: &str) -> Result<Vec<(PathBuf, Found)>, Failure> {
    let mut dirs: Vec<(PathBuf, Found)> = vec![(context.default_of(agent).to_path_buf(), Found::Default)];
    let mut add = |dir: PathBuf, found: Found| {
        if !dirs.iter().any(|(known, _)| crate::homes::same_dir(known, &dir)) {
            dirs.push((dir, found));
        }
    };
    for login in &kept.of(agent).logins {
        add(login.dir.clone(), login.found);
    }
    let chosen = kept::chosen(node).map_err(|error| Failure::new(format!("reading the chosen logins: {error}")))?;
    for one in chosen.added.into_iter().filter(|one| one.agent.as_str() == agent) {
        let found = if in_folder(&chosen.folders, &one.dir) { Found::Elsewhere } else { Found::Added };
        add(one.dir, found);
    }
    let integrations = apply::integrations(&context.places)
        .map_err(|error| Failure::new(format!("reading the apply record: {error}")))?;
    let wanted = host(agent)?;
    for integration in integrations.into_iter().filter(|integration| integration.host == wanted) {
        add(fastctx_hosts::inventory::plain(&integration.home), Found::Added);
    }
    Ok(dirs)
}

/// Whether `dir` lies below one of the chosen `folders`: a login taken up
/// from what a folder's search found. Paths are matched by their text, as
/// the search wrote them: it follows no link, so none needs resolving.
fn in_folder(folders: &[PathBuf], dir: &Path) -> bool {
    let dir = walk::key(dir);
    folders.iter().any(|folder| folders::below(&dir, &walk::key(folder)))
}

/// Whether the World's tools are in the login `dir` of `host`.
fn standing(context: &Context, host: Host, dir: &Path) -> Result<Integration, String> {
    apply::status_at(&context.places, host, dir).map_err(|error| error.to_string()).map(|standing| match standing {
        Standing::Absent => Integration::Absent,
        Standing::Out => Integration::Out,
        Standing::In => Integration::In,
        Standing::Old => Integration::Old,
    })
}

/// The most specific login whose sessions a Claude transcript is among.
/// The Claude Code login a session's transcript is kept in, among those the
/// World knows here; the default one when it is in none of them.
pub(crate) fn claude_login_of(node: &Node, transcript: &Path) -> Result<PathBuf, Failure> {
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    let homes: Vec<AgentHome> = dirs(node, &context, &kept, "claude")?
        .into_iter()
        .map(|(dir, _)| AgentHome { dir, found: HomeFound::Added })
        .collect();
    Ok(owner(&homes, transcript).map(Path::to_path_buf).unwrap_or(context.claude_default))
}

fn owner<'a>(homes: &'a [AgentHome], transcript: &Path) -> Option<&'a Path> {
    crate::homes::limits_home(homes, transcript).map(|home| home.dir.as_path())
}

/// The limits of the Claude login `dir` as its sessions last read them:
/// Claude Code reads them only in a session at work, and its usage ledger
/// keeps the last reading past that session and its conversation.
fn claude_limits(node: &Node, dir: &Path) -> Fact<AgentLimits> {
    match crate::usage::last_limits(node, "claude", dir) {
        Some(limits) => Fact::known(limits),
        None => Fact::unavailable("no session in this login has read its limits yet"),
    }
}

/// Turns the automatic naming of new conversations in a Codex login here on
/// or off. Known only for Codex; a login the scan does not yet list is added
/// to the record so the setting is kept.
pub(crate) fn set_title(node: &Node, agent: &AgentName, dir: &Path, on: bool) -> Result<(), Failure> {
    if agent.as_str() != "codex" {
        return Err(Failure::not_run("only Codex names its own conversations; Claude Code does its own"));
    }
    known(node, agent, dir)?;
    let dir = dir.to_path_buf();
    kept::change(node, move |kept| {
        kept.codex.untitled.retain(|known| !crate::homes::same_dir(known, &dir));
        if !on {
            kept.codex.untitled.push(dir);
        }
    })
    .map_err(|error| Failure::new(format!("keeping the login's naming: {error}")))
}

/// Turns on or off whether a week the vendor cleared before its reset counts
/// anew in the Codex or Claude Code login `dir` here, as its owner chooses;
/// off by default (`world-usage.md`).
pub(crate) fn set_resets_in_place(node: &Node, agent: &AgentName, dir: &Path, on: bool) -> Result<(), Failure> {
    if !AGENTS.contains(&agent.as_str()) {
        return Err(Failure::not_run("only Codex and Claude Code logins keep their usage"));
    }
    known(node, agent, dir)?;
    crate::usage::set_resets_in_place(node, agent.as_str(), dir, on)
        .map_err(|error| Failure::new(format!("keeping the login's weekly resets: {error}")))?;
    snapshot::ask(node);
    Ok(())
}

/// Whether the Codex login `dir` (the default one when none) names its new
/// conversations: on unless turned off there. Quiet about any trouble
/// reading the record: unread is the default, on.
pub(crate) fn codex_auto_title(node: &Node, dir: Option<&Path>) -> bool {
    let Ok(kept) = kept::read(node) else { return true };
    let default = node.codex.default().config_home().to_path_buf();
    let dir = dir.unwrap_or(&default);
    !kept.codex.untitled.iter().any(|known| crate::homes::same_dir(known, dir))
}

/// The login `dir` of `agent` is one the World knows here, listed or a
/// candidate; it must still be a directory.
pub(crate) fn known(node: &Node, agent: &AgentName, dir: &Path) -> Result<(), Failure> {
    let other = fastctx_install::agents::find(agent.as_str());
    if other.is_none() {
        host(agent.as_str())?;
    }
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    let dirs = match other {
        Some(other) => others::dirs(&context, &kept, other)?,
        None => dirs(node, &context, &kept, agent.as_str())?,
    };
    if !dirs.iter().any(|(known, _)| crate::homes::same_dir(known, dir)) {
        return Err(Failure::not_run(format!(
            "{} is not a {agent} login known on {}; add it first",
            dir.display(),
            node.machine()
        )));
    }
    if !fastctx_platform::disk::is_dir(dir) {
        return Err(Failure::not_run(format!("{} is not a directory on {}", dir.display(), node.machine())));
    }
    Ok(())
}

/// Whether the World's tools are in `agent`'s login `dir` here, `None`
/// being the agent's default one: only such a login is the World's to use.
/// Answers the login as a session records it: `None` for the default one.
pub(crate) fn listed(node: &Node, agent: &str, dir: Option<&Path>) -> Result<Option<PathBuf>, Failure> {
    let context = Context::of(node)?;
    let wanted = host(agent)?;
    let default = context.default_of(agent).to_path_buf();
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(|| default.clone());
    let integration = standing(&context, wanted, &dir)
        .map_err(|error| Failure::new(format!("reading how {} stands: {error}", dir.display())))?;
    if integration != Integration::In {
        return Err(Failure::not_run(format!(
            "the {agent} login {} on {} is not in its agent list: the World uses a login once its tools are put into it on the agents page",
            dir.display(),
            node.machine()
        )));
    }
    if !fastctx_platform::disk::is_dir(&dir) {
        return Err(Failure::not_run(format!("{} is no longer a directory on {}", dir.display(), node.machine())));
    }
    Ok((!crate::homes::same_dir(&dir, &default)).then_some(dir))
}

/// The record of `machine`, or of this one and every machine the asker may
/// use, each as it keeps it.
pub(crate) async fn query(
    node: &Arc<Node>,
    acting: &fastctx_types::access::Acting,
    machine: Option<Written>,
) -> Result<fastctx_types::logins::WorldLogins, Failure> {
    remote::query(node, acting, machine).await
}

/// Takes up `dir` as a candidate login of `agent`: a folder the user chose
/// that is a login, or one a folder's search found. Who is logged in there
/// is read (`identify::asked`), never its limits; the directory is kept
/// among those the user took up.
pub(crate) async fn take_up(node: &Arc<Node>, agent: AgentName, dir: PathBuf) -> Result<MachineLogins, Failure> {
    let agent: &'static str = match host(agent.as_str())? {
        Host::Codex => "codex",
        Host::Claude => "claude",
    };
    if !dir.is_absolute() {
        return Err(Failure::not_run(format!("{} is not an absolute path", dir.display())));
    }
    let (found, default, programs, signed, credential, env) = {
        let (node, dir) = (node.clone(), dir.clone());
        fastctx_platform::task::spawn_blocking(move || {
            if !fastctx_platform::disk::is_dir(&dir) {
                return Err(Failure::not_run(format!("{} is not a directory on {}", dir.display(), node.machine())));
            }
            let context = Context::of(&node)?;
            let kept =
                kept::read(&node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
            let chosen =
                kept::folders(&node).map_err(|error| Failure::new(format!("reading the chosen folders: {error}")))?;
            let name = AgentName::try_from(agent).expect("an agent's name");
            kept::add(&node, &name, &dir).map_err(|error| Failure::new(format!("keeping the added login: {error}")))?;
            let default = crate::homes::same_dir(&dir, context.default_of(agent));
            let found = if default {
                Found::Default
            } else if in_folder(&chosen, &dir) {
                Found::Elsewhere
            } else {
                Found::Added
            };
            let signed = sign_in::now(&context, agent, &dir);
            let credential = sign_in::credential(&context, agent, &dir);
            Ok::<_, Failure>((found, default, kept.of(agent).programs.clone(), signed, credential, context.env))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    };
    let asked = identify::asked(node, agent, &dir, default, programs, env).await;
    {
        let (node, dir) = (node.clone(), dir.clone());
        fastctx_platform::task::spawn_blocking(move || {
            kept::change(&node, |kept| {
                let logins = &mut kept.of_mut(agent).logins;
                let before = logins
                    .iter()
                    .position(|known| crate::homes::same_dir(&known.dir, &dir))
                    .map(|at| logins.remove(at));
                logins.retain(|known| !crate::homes::same_dir(&known.dir, &dir));
                logins.push(kept::KeptLogin::asked(dir, found, asked, signed, credential, before.as_ref()));
            })
            .map_err(|error| Failure::new(format!("keeping the login: {error}")))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    }
    changed_here(node).await
}

/// Claude configuration directories already known on this machine.
pub(crate) fn claude_directories(node: &Node) -> Result<Vec<PathBuf>, Failure> {
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|e| Failure::new(e.to_string()))?;
    Ok(dirs(node, &context, &kept, "claude")?.into_iter().map(|(dir, _)| dir).collect())
}
