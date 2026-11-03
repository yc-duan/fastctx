//! The logins of the agents connected through their configuration alone
//! (`fastctx_install::agents`, `world-layers.md`, 支持尽可能多的智能体): a
//! home each, where the agent keeps one, connected or not, and who its
//! files say is logged in. Nothing is asked of the agent's program and
//! nothing goes over the network; their limits and usage are the
//! workbench's, which they have none of.

use std::path::{Path, PathBuf};

use fastctx_install::agents::Agent;
use fastctx_install::apply;
use fastctx_types::AgentName;
use fastctx_types::agents::{Fact, Integration};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{AgentLogins, Found, Identified, Login, LoginChange};
use jiff::Timestamp;

use super::Context;
use super::kept::{Kept, KeptLogin};

/// The home the agent keeps by default, as the user's environment places it.
pub(super) fn default_home(context: &Context, agent: &Agent) -> PathBuf {
    agent.home.resolve(&context.user_home, |name| {
        context.env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.clone())
    })
}

/// Each home of `agent` the World knows here: its default one where it is
/// there, those the last scan found, and those Fastctx is connected to.
pub(super) fn dirs(context: &Context, kept: &Kept, agent: &Agent) -> Result<Vec<(PathBuf, Found)>, Failure> {
    let mut dirs: Vec<(PathBuf, Found)> = Vec::new();
    let mut add = |dir: PathBuf, found: Found| {
        if !dirs.iter().any(|(known, _)| crate::homes::same_dir(known, &dir)) {
            dirs.push((dir, found));
        }
    };
    let home = default_home(context, agent);
    if fastctx_platform::disk::is_dir(&home) {
        add(home, Found::Default);
    }
    for login in &kept.of(agent.name).logins {
        add(login.dir.clone(), login.found);
    }
    let integrations = apply::agent_integrations(&context.places)
        .map_err(|error| Failure::new(format!("reading the agents' records: {error}")))?;
    for (_, home) in integrations.into_iter().filter(|(name, _)| name == agent.name) {
        add(fastctx_hosts::inventory::plain(&home), Found::Added);
    }
    Ok(dirs)
}

/// What a scan keeps of `agent`: its default home, where it is there, with
/// who its files say is logged in.
pub(super) fn scanned(context: &Context, agent: &Agent) -> Vec<KeptLogin> {
    let home = default_home(context, agent);
    if !fastctx_platform::disk::is_dir(&home) {
        return Vec::new();
    }
    vec![asked(home, Found::Default, agent)]
}

fn asked(dir: PathBuf, found: Found, agent: &Agent) -> KeptLogin {
    let who = match (agent.identity)(&dir) {
        Some(identity) => Fact::known(identity),
        None => Fact::unavailable("its files name no account"),
    };
    KeptLogin {
        dir,
        found,
        identified: Identified { at: Timestamp::now(), who },
        launcher: None,
        asked_file: None,
        models: Vec::new(),
        signed: None,
    }
}

/// The agent's logins as the interface lists them: connected ones listed,
/// the rest candidates, and what a connected one waits for the user for.
pub(super) fn side(
    node: &crate::Node,
    context: &Context,
    kept: &Kept,
    agent: &Agent,
    changes: &[(AgentName, PathBuf, LoginChange)],
) -> Result<AgentLogins, Failure> {
    let mut side = AgentLogins { title: Some(agent.title.to_owned()), ..AgentLogins::default() };
    let default = default_home(context, agent);
    for (dir, found) in dirs(context, kept, agent)? {
        let connected = apply::agent_connected(&context.places, agent, &dir)
            .map_err(|error| Failure::new(format!("reading how {} stands: {error}", dir.display())))?;
        let known = kept.of(agent.name).logins.iter().find(|login| crate::homes::same_dir(&login.dir, &dir));
        let login = Login {
            default: crate::homes::same_dir(&dir, &default),
            found,
            identity: Some(
                known
                    .map(|login| login.identified.clone())
                    .unwrap_or_else(|| asked(dir.clone(), found, agent).identified),
            ),
            limits: Fact::unavailable("only the workbench reads limits"),
            programs: Vec::new(),
            integration: if connected { Integration::In } else { Integration::Absent },
            unattended: None,
            auto_title: false,
            launcher: None,
            models: Vec::new(),
            change: change_of(changes, agent, &dir),
            drift: if connected { super::refresh::of(node, agent.name, &dir) } else { None },
            workbench: false,
            dir,
        };
        if connected {
            side.listed.push(login);
        } else {
            side.candidates.push(login);
        }
    }
    Ok(side)
}

fn change_of(changes: &[(AgentName, PathBuf, LoginChange)], agent: &Agent, dir: &Path) -> Option<LoginChange> {
    changes
        .iter()
        .find(|(owner, path, _)| owner.as_str() == agent.name && crate::homes::same_dir(path, dir))
        .map(|(_, _, change)| change.clone())
}
