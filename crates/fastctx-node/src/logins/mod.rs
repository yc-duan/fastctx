//! This machine's agents' logins, as its node keeps them
//! (`world-agent-logins.md`): which the World lists and uses, which are
//! candidates, what a scan found, the folders the user chose and what their
//! searches found, the limits last read, and the changes of the list under
//! way. A scan, a search, a change or a fresh read of limits happens only
//! when the user asks for it, here or from another of their devices.
//! Reading the record starts no process, but to ask again a login whose
//! sign-in changed since it was asked (`sign_in`); which program each login
//! starts is read as starting it reads it (`program`), and what its agent
//! says is kept fresh as the World starts it for its own work (`heard`).
//!
//! The list is exactly the logins the World's tools are put into: the apply
//! record (`applied.json`) says which, so the list and what the user's own
//! sessions get can never disagree.

pub(crate) mod change;
pub(crate) mod folders;
pub(crate) mod heard;
pub(crate) mod identify;
pub(crate) mod kept;
pub(crate) mod limits;
mod program;
pub(crate) mod refresh;
pub(crate) mod remote;
pub(crate) mod scan;
mod sign_in;
pub(crate) mod watch;

use fastctx_types::access::Written;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use fastctx_hosts::inventory::walk;
use fastctx_install::apply::{self, Host, Standing};
use fastctx_types::AgentName;
use fastctx_types::agents::{AgentHome, AgentLimits, Fact, HomeFound, Integration};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{AgentLogins, Found, Login, LoginChange, MachineLogins, Stage};

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

/// The record of this machine's logins, each login whose sign-in changed
/// since it was asked asked again first.
pub(crate) async fn here(node: &Arc<Node>) -> Result<MachineLogins, Failure> {
    sign_in::ask_changed(node).await;
    let node = node.clone();
    tokio::task::spawn_blocking(move || assemble(&node))
        .await
        .map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?
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

fn assemble(node: &Node) -> Result<MachineLogins, Failure> {
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    let mut scan = kept.scan.clone();
    // A scan the record says runs, with none running here, was cut short
    // when the node stopped.
    if let Some(scan) = &mut scan
        && scan.usual == Stage::Running
        && !node.logins.scanning.load(Ordering::Acquire)
    {
        scan.usual = Stage::Failed { at: jiff::Timestamp::now(), reason: "the node stopped during it".into() };
    }
    let claude_models = fastctx_hosts::claude::exchange::models(node.world.path())
        .map_err(|error| Failure::new(format!("reading Claude's model rows: {error}")))?;
    let changes = node.logins.changes.lock().expect("never poisoned").clone();
    let mut logins = MachineLogins {
        machine: node.machine(),
        scan,
        codex: AgentLogins::default(),
        claude: AgentLogins::default(),
        folders: Vec::new(),
    };
    let mut known = HashSet::new();
    for agent in AGENTS {
        let dirs = dirs(node, &context, &kept, agent)?;
        let side = logins.of_mut(agent).expect("an agent that keeps logins");
        side.programs = kept.of(agent).programs.clone();
        let homes: Vec<AgentHome> =
            dirs.iter().map(|(dir, _)| AgentHome { dir: dir.clone(), found: HomeFound::Added }).collect();
        for (dir, found) in &dirs {
            let default = *found == Found::Default;
            let known = kept.of(agent).logins.iter().find(|login| crate::homes::same_dir(&login.dir, dir));
            let integration = match standing(&context, host(agent)?, dir) {
                Ok(integration) => integration,
                Err(error) => return Err(Failure::new(format!("reading how {} stands: {error}", dir.display()))),
            };
            let listed = integration == Integration::In;
            let limits = if agent == "codex" { limits::of(node, &kept, dir) } else { claude_limits(node, dir) };
            let models = if agent == "codex" {
                known.map(|login| login.models.clone()).unwrap_or_default()
            } else {
                claude_models
                    .iter()
                    .find(|(transcript, _, _)| {
                        owner(&homes, transcript).is_some_and(|home| crate::homes::same_dir(home, dir))
                    })
                    .map(|(_, _, models)| models.clone())
                    .unwrap_or_else(|| fastctx_hosts::claude::models::agent_models(None))
            };
            // The permission is read only where the World's tools are in,
            // which granted it: a candidate's settings are not the World's
            // to read.
            let unattended = if agent == "claude" && listed {
                Some(apply::claude_consent_at(&context.places, dir).map_err(|error| {
                    Failure::new(format!("reading Claude Code's permission in {}: {error}", dir.display()))
                })?)
            } else {
                None
            };
            let change = changes
                .iter()
                .find(|(owner, path, _)| owner.as_str() == agent && crate::homes::same_dir(path, dir))
                .map(|(_, _, change)| change.clone());
            let login = Login {
                dir: dir.clone(),
                default,
                found: *found,
                identity: known.map(|login| login.identified.clone()),
                limits,
                programs: if default { (0..side.programs.len() as u32).collect() } else { Vec::new() },
                integration,
                unattended,
                auto_title: agent == "codex"
                    && !kept.codex.untitled.iter().any(|known| crate::homes::same_dir(known, dir)),
                launcher: program::now(node, agent, dir, known),
                models,
                change,
                drift: if listed { refresh::of(node, agent, dir) } else { None },
            };
            if listed {
                side.listed.push(login);
            } else {
                side.candidates.push(login);
            }
        }
        known.extend(dirs.iter().map(|(dir, _)| walk::key(dir)));
    }
    let chosen = kept::folders(node).map_err(|error| Failure::new(format!("reading the chosen folders: {error}")))?;
    logins.folders = folders::shown(node, &kept, &chosen, &known);
    Ok(logins)
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
    host(agent.as_str())?;
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    let dirs = dirs(node, &context, &kept, agent.as_str())?;
    if !dirs.iter().any(|(known, _)| crate::homes::same_dir(known, dir)) {
        return Err(Failure::not_run(format!(
            "{} is not a {agent} login known on {}; add it first",
            dir.display(),
            node.machine()
        )));
    }
    if !dir.is_dir() {
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
    if !dir.is_dir() {
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
/// that is a login, or one a folder's search found. The agent is asked who
/// is logged in there, not its limits; the directory is kept among those
/// the user took up.
pub(crate) async fn take_up(node: &Arc<Node>, agent: AgentName, dir: PathBuf) -> Result<MachineLogins, Failure> {
    host(agent.as_str())?;
    if !dir.is_absolute() {
        return Err(Failure::not_run(format!("{} is not an absolute path", dir.display())));
    }
    let (found, default, programs, signed) = {
        let (node, agent, dir) = (node.clone(), agent.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            if !dir.is_dir() {
                return Err(Failure::not_run(format!("{} is not a directory on {}", dir.display(), node.machine())));
            }
            let context = Context::of(&node)?;
            let kept =
                kept::read(&node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
            let chosen =
                kept::folders(&node).map_err(|error| Failure::new(format!("reading the chosen folders: {error}")))?;
            kept::add(&node, &agent, &dir)
                .map_err(|error| Failure::new(format!("keeping the added login: {error}")))?;
            let default = crate::homes::same_dir(&dir, context.default_of(agent.as_str()));
            let found = if default {
                Found::Default
            } else if in_folder(&chosen, &dir) {
                Found::Elsewhere
            } else {
                Found::Added
            };
            let signed = sign_in::now(&context, agent.as_str(), &dir);
            Ok::<_, Failure>((found, default, kept.claude.programs.clone(), signed))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    };
    let asked = if agent.as_str() == "codex" {
        identify::codex(node, &dir, false).await
    } else {
        let path = dir.clone();
        tokio::task::spawn_blocking(move || identify::claude(&path, default, &programs))
            .await
            .map_err(|error| Failure::new(error.to_string()))??
    };
    {
        let (node, agent, dir) = (node.clone(), agent.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            kept::change(&node, |kept| {
                let logins = &mut kept.of_mut(agent.as_str()).logins;
                logins.retain(|known| !crate::homes::same_dir(&known.dir, &dir));
                logins.push(kept::KeptLogin::asked(dir, found, asked, signed));
            })
            .map_err(|error| Failure::new(format!("keeping the login: {error}")))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    }
    here(node).await
}

/// Claude configuration directories already known on this machine.
pub(crate) fn claude_directories(node: &Node) -> Result<Vec<PathBuf>, Failure> {
    let context = Context::of(node)?;
    let kept = kept::read(node).map_err(|e| Failure::new(e.to_string()))?;
    Ok(dirs(node, &context, &kept, "claude")?.into_iter().map(|(dir, _)| dir).collect())
}
