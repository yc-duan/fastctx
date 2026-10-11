//! This machine's agents' logins, as its node keeps them
//! (`world-agent-logins.md`): which the World lists and uses, which are
//! candidates, what a scan found and how far it has got, the limits last
//! read, and the changes of the list under way. Reading the record starts
//! no process; a scan, a change or a fresh read of limits happens only when
//! the user asks for it, here or from another of their devices.
//!
//! The list is exactly the logins the World's tools are put into: the apply
//! record (`applied.json`) says which, so the list and what the user's own
//! sessions get can never disagree.

pub(crate) mod change;
pub(crate) mod identify;
pub(crate) mod kept;
pub(crate) mod limits;
pub(crate) mod remote;
pub(crate) mod scan;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_install::apply::{self, Host, Standing};
use fastctx_types::agents::{AgentHome, AgentLimits, Fact, HomeFound, Integration};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{AgentLogins, Found, Login, LoginChange, MachineLogins, Stage};
use fastctx_types::{AgentName, MachineName};

use crate::Node;

/// The node's state of its logins beyond what is kept on disk.
#[derive(Default)]
pub(crate) struct Logins {
    /// Writing the kept record, one at a time.
    keeping: Mutex<()>,
    /// The scan under way, if one is.
    scan: Mutex<Option<Arc<scan::Running>>>,
    /// The change running in each login, or the last one since the node
    /// started: its agent, directory and steps.
    changes: Mutex<Vec<(AgentName, PathBuf, LoginChange)>>,
    /// Changes of the list happen one at a time, as `fastctx apply` does.
    changing: tokio::sync::Mutex<()>,
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
        let places = crate::agents::places(node, &env, &user_home, identify::claude_program());
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

/// The record of this machine's logins.
pub(crate) async fn here(node: &Arc<Node>) -> Result<MachineLogins, Failure> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || assemble(&node))
        .await
        .map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?
}

/// Each login of `agent` the World knows here, with how it knows of it:
/// the default one, those the last scan found or the user took up, those
/// the user added, and those the World's tools are put into.
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
    let added = kept::added(node).map_err(|error| Failure::new(format!("reading the added logins: {error}")))?;
    for one in added.into_iter().filter(|one| one.agent.as_str() == agent) {
        let elsewhere = kept.of(agent).elsewhere.iter().any(|found| crate::homes::same_dir(&found.dir, &one.dir));
        add(one.dir, if elsewhere { Found::Elsewhere } else { Found::Added });
    }
    let integrations = apply::integrations(&context.places)
        .map_err(|error| Failure::new(format!("reading the apply record: {error}")))?;
    let wanted = host(agent)?;
    for integration in integrations.into_iter().filter(|integration| integration.host == wanted) {
        add(fastctx_hosts::inventory::plain(&integration.home), Found::Added);
    }
    Ok(dirs)
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
    let running = node.logins.scan.lock().expect("never poisoned").clone();
    let mut scan = kept.scan.clone();
    match (&running, &mut scan) {
        (Some(running), Some(scan)) => running.progress(scan),
        // A scan the record says runs, with none running here, was cut
        // short when the node stopped.
        (None, Some(scan)) => {
            let at = jiff::Timestamp::now();
            let cut = |stage: &mut Stage| {
                if *stage == Stage::Running {
                    *stage = Stage::Failed { at, reason: "the node stopped during it".into() };
                }
            };
            cut(&mut scan.usual);
            if let Some(disks) = &mut scan.disks {
                cut(&mut disks.stage);
            }
        }
        _ => {}
    }
    let claude_limits = fastctx_hosts::claude::exchange::limits(node.world.path());
    let claude_models = fastctx_hosts::claude::exchange::models(node.world.path())
        .map_err(|error| Failure::new(format!("reading Claude's model rows: {error}")))?;
    let changes = node.logins.changes.lock().expect("never poisoned").clone();
    let mut logins =
        MachineLogins { machine: node.machine(), scan, codex: AgentLogins::default(), claude: AgentLogins::default() };
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
            let limits =
                if agent == "codex" { limits::of(&kept, dir) } else { claude_reading(&homes, dir, &claude_limits) };
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
                launcher: known.and_then(|login| login.launcher.clone()),
                models,
                change,
            };
            if listed {
                side.listed.push(login);
            } else {
                side.candidates.push(login);
            }
        }
        side.elsewhere = kept
            .of(agent)
            .elsewhere
            .iter()
            .filter(|found| !dirs.iter().any(|(dir, _)| crate::homes::same_dir(dir, &found.dir)))
            .cloned()
            .collect();
    }
    Ok(logins)
}

/// The most specific login whose sessions a Claude transcript is among.
fn owner<'a>(homes: &'a [AgentHome], transcript: &Path) -> Option<&'a Path> {
    crate::homes::limits_home(homes, transcript).map(|home| home.dir.as_path())
}

/// The newest reading of the limits of the Claude login `dir`, from its
/// sessions' reports.
fn claude_reading(homes: &[AgentHome], dir: &Path, readings: &[(PathBuf, AgentLimits)]) -> Fact<AgentLimits> {
    match readings
        .iter()
        .find(|(transcript, _)| owner(homes, transcript).is_some_and(|home| crate::homes::same_dir(home, dir)))
    {
        Some((_, limits)) => Fact::known(limits.clone()),
        None => Fact::unavailable("no session in this login has reported its limits yet"),
    }
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
    machine: Option<MachineName>,
) -> Result<fastctx_types::logins::WorldLogins, Failure> {
    remote::query(node, acting, machine).await
}

/// Takes up `dir` as a candidate login of `agent`: one the user wrote, or
/// one the walk of every disk found. The agent is asked who is logged in
/// there, and the directory is kept among those the user added.
pub(crate) async fn take_up(node: &Arc<Node>, agent: AgentName, dir: PathBuf) -> Result<MachineLogins, Failure> {
    host(agent.as_str())?;
    if !dir.is_absolute() {
        return Err(Failure::not_run(format!("{} is not an absolute path", dir.display())));
    }
    if !dir.is_dir() {
        return Err(Failure::not_run(format!("{} is not a directory on {}", dir.display(), node.machine())));
    }
    let (found, default, programs) = {
        let (node, agent, dir) = (node.clone(), agent.clone(), dir.clone());
        tokio::task::spawn_blocking(move || {
            let context = Context::of(&node)?;
            let kept =
                kept::read(&node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
            let elsewhere =
                kept.of(agent.as_str()).elsewhere.iter().any(|found| crate::homes::same_dir(&found.dir, &dir));
            kept::add(&node, &agent, &dir)
                .map_err(|error| Failure::new(format!("keeping the added login: {error}")))?;
            let default = crate::homes::same_dir(&dir, context.default_of(agent.as_str()));
            let found = if default {
                Found::Default
            } else if elsewhere {
                Found::Elsewhere
            } else {
                Found::Added
            };
            Ok::<_, Failure>((found, default, kept.claude.programs.clone()))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    };
    let asked = if agent.as_str() == "codex" {
        identify::codex(node, &dir).await
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
                if let Some((at, answer)) = asked.limits {
                    let limits = &mut kept.codex.limits;
                    limits.retain(|known| !crate::homes::same_dir(&known.dir, &dir));
                    limits.push(kept::KeptLimits {
                        dir: dir.clone(),
                        at,
                        buckets: fastctx_hosts::codex::buckets(&answer),
                    });
                }
                let logins = &mut kept.of_mut(agent.as_str()).logins;
                logins.retain(|known| !crate::homes::same_dir(&known.dir, &dir));
                logins.push(kept::KeptLogin {
                    dir,
                    found,
                    identified: asked.identified,
                    launcher: asked.launcher,
                    models: asked.models,
                });
            })
            .map_err(|error| Failure::new(format!("keeping the login: {error}")))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))??;
    }
    here(node).await
}
