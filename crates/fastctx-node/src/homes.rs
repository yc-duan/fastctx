//! The agents' homes on this machine, one account apiece (`AgentHome`).
//! Neither agent keeps a list of them, so the node finds them: each agent's
//! default home, the directories directly in the user's home whose names
//! begin like it and that hold that agent's things, and those the user
//! added; minus those the user hid. What the user changed is kept in
//! `<world>/agent-homes.json`.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_platform::{Permissions, publish};
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::agents::{AgentHome, HomeChange, HomeFound};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AgentName, MachineName};
use serde::{Deserialize, Serialize};

use crate::Node;

/// The agents that keep homes, each with the start of its directories'
/// names and what in a directory shows it is that agent's: only whether
/// these are there is looked at, never what they hold.
const AGENTS: [(&str, &str, &[&str]); 2] = [
    ("codex", ".codex", &["config.toml", "auth.json", "sessions"]),
    ("claude", ".claude", &["settings.json", ".credentials.json", "projects", ".claude.json", "daemon.json"]),
];

#[derive(Default, Serialize, Deserialize)]
struct Changes {
    #[serde(default)]
    added: Vec<Kept>,
    #[serde(default)]
    hidden: Vec<Kept>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    agent: AgentName,
    dir: PathBuf,
}

fn path(node: &Node) -> PathBuf {
    node.world.path().join("agent-homes.json")
}

fn read(node: &Node) -> io::Result<Changes> {
    match std::fs::read(path(node)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("agent-homes.json: {error}"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Changes::default()),
        Err(error) => Err(error),
    }
}

/// Whether two directories are the same home, as this machine writes
/// paths: on Windows letters' case and either slash do not matter.
pub(crate) fn same_dir(a: &Path, b: &Path) -> bool {
    normal(a) == normal(b)
}

/// Whether `path` lies inside `dir`, as this machine writes paths.
pub(crate) fn within(dir: &Path, path: &Path) -> bool {
    normal(path).starts_with(&format!("{}/", normal(dir)))
}

fn normal(dir: &Path) -> String {
    let text = dir.to_string_lossy();
    let text = if cfg!(windows) { text.replace('\\', "/").to_lowercase() } else { text.into_owned() };
    text.trim_end_matches('/').to_string()
}

/// A short name for a home's own directories in the World's, the same for
/// every way of writing it.
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

/// Every home on this machine the user has not hidden, each agent's
/// default first.
pub(crate) fn list(node: &Node, env: &[(OsString, OsString)], user_home: &Path) -> io::Result<Vec<(AgentName, AgentHome)>> {
    let changes = read(node)?;
    let hidden = |agent: &str, dir: &Path| {
        changes.hidden.iter().any(|kept| kept.agent.as_str() == agent && same_dir(&kept.dir, dir))
    };
    let mut homes: Vec<(AgentName, AgentHome)> = Vec::new();
    let mut add = |agent: &str, dir: PathBuf, found: HomeFound| {
        let known = homes.iter().any(|(known, home)| known.as_str() == agent && same_dir(&home.dir, &dir));
        if !known && (found == HomeFound::Default || !hidden(agent, &dir)) {
            homes.push((agent.parse().expect("an agent name"), AgentHome { dir, found }));
        }
    };
    add("codex", node.codex.default().config_home().to_path_buf(), HomeFound::Default);
    add("claude", claude_default(env, user_home), HomeFound::Default);
    let mut found = Vec::new();
    for entry in std::fs::read_dir(user_home)?.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        for (agent, prefix, marks) in AGENTS {
            if name.starts_with(prefix) && dir.is_dir() && marks.iter().any(|mark| dir.join(mark).exists()) {
                found.push((agent, dir.clone()));
            }
        }
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    for (agent, dir) in found {
        add(agent, dir, HomeFound::Found);
    }
    for kept in &changes.added {
        add(kept.agent.as_str(), kept.dir.clone(), HomeFound::Added);
    }
    Ok(homes)
}

/// The home of `agent` written `dir`, if this machine has it; `None` is the
/// default one.
pub(crate) fn find(
    node: &Node,
    env: &[(OsString, OsString)],
    user_home: &Path,
    agent: &str,
    dir: Option<&Path>,
) -> Result<AgentHome, Failure> {
    let homes = list(node, env, user_home).map_err(|error| Failure::new(format!("listing the agents' homes: {error}")))?;
    let found = homes.into_iter().find(|(known, home)| {
        known.as_str() == agent
            && match dir {
                None => home.found == HomeFound::Default,
                Some(dir) => same_dir(&home.dir, dir),
            }
    });
    found.map(|(_, home)| home).ok_or_else(|| {
        Failure::new(format!(
            "{agent} has no home {} on {}",
            dir.map(|dir| dir.display().to_string()).unwrap_or_default(),
            node.machine()
        ))
    })
}

/// What the user changes of the homes `machine` knows, carried there.
pub(crate) async fn call(node: &Arc<Node>, machine: MachineName, change: HomeChange) -> Result<(), Failure> {
    if machine == node.machine() {
        return self::change(node, change);
    }
    let call = RemoteCall::Homes { change };
    match node
        .call(&Acting::Here, &(&machine).into(), Purpose::Agents, call)
        .await
        .map_err(|error| Failure::new(error.to_string()))??
    {
        RemoteReply::Set => Ok(()),
        _ => Err(Failure::new(format!("{machine} answered something other than its agents' homes"))),
    }
}

/// What the user changes of the homes the node knows.
pub(crate) fn change(node: &Node, change: HomeChange) -> Result<(), Failure> {
    let mut changes = read(node).map_err(|error| Failure::new(format!("reading the agents' homes: {error}")))?;
    let (agent, dir) = match &change {
        HomeChange::Add { agent, dir } | HomeChange::Hide { agent, dir } | HomeChange::Show { agent, dir } => {
            (agent.clone(), dir.clone())
        }
    };
    if !AGENTS.iter().any(|(known, _, _)| *known == agent.as_str()) {
        return Err(Failure::new(format!("{agent} keeps no homes the World knows of")));
    }
    let same = |kept: &Kept| kept.agent == agent && same_dir(&kept.dir, &dir);
    match change {
        HomeChange::Add { .. } => {
            if !dir.is_absolute() {
                return Err(Failure::new(format!("{} is not an absolute path", dir.display())));
            }
            if !dir.is_dir() {
                return Err(Failure::new(format!("{} is not a directory on {}", dir.display(), node.machine())));
            }
            changes.hidden.retain(|kept| !same(kept));
            if !changes.added.iter().any(same) {
                changes.added.push(Kept { agent, dir });
            }
        }
        HomeChange::Hide { .. } => {
            changes.added.retain(|kept| !same(kept));
            if !changes.hidden.iter().any(same) {
                changes.hidden.push(Kept { agent, dir });
            }
        }
        HomeChange::Show { .. } => changes.hidden.retain(|kept| !same(kept)),
    }
    let bytes = serde_json::to_vec_pretty(&changes).expect("homes serialize");
    publish(&path(node), &bytes, Permissions::Private)
        .map_err(|error| Failure::new(format!("keeping the agents' homes: {error}")))
}
