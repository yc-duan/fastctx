//! What the node keeps on disk of its agents' logins: the last scan's
//! findings in `<world>/agent-scan.json`, each login's limits as last read
//! with them, and the directories the user added in `<world>/agent-homes.json`
//! (`world-agent-logins.md`). Nothing in either is a credential: who is
//! logged in, as the agents said it, and paths.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};
use fastctx_types::AgentName;
use fastctx_types::agents::AgentModel;
use fastctx_types::logins::{Found, FoundProgram, Identified, Launcher, LooksLike, Scan};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Kept {
    #[serde(default)]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,
    #[serde(default)]
    pub codex: KeptAgent,
    #[serde(default)]
    pub claude: KeptAgent,
}

impl Kept {
    pub fn of(&self, agent: &str) -> &KeptAgent {
        if agent == "codex" { &self.codex } else { &self.claude }
    }

    pub fn of_mut(&mut self, agent: &str) -> &mut KeptAgent {
        if agent == "codex" { &mut self.codex } else { &mut self.claude }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct KeptAgent {
    #[serde(default)]
    pub programs: Vec<FoundProgram>,
    /// Each login asked who it is, by the last scan or when taken up.
    #[serde(default)]
    pub logins: Vec<KeptLogin>,
    #[serde(default)]
    pub elsewhere: Vec<LooksLike>,
    /// Codex's limits as last read or pushed, for each login.
    #[serde(default)]
    pub limits: Vec<KeptLimits>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct KeptLogin {
    pub dir: PathBuf,
    pub found: Found,
    pub identified: Identified,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AgentModel>,
}

/// A Codex login's limits as Codex keys them, merged as it pushes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct KeptLimits {
    pub dir: PathBuf,
    pub at: Timestamp,
    pub buckets: BTreeMap<String, serde_json::Value>,
}

fn path(node: &Node) -> PathBuf {
    node.world.path().join("agent-scan.json")
}

/// The record as last kept.
pub(super) fn read(node: &Node) -> io::Result<Kept> {
    let kept: Kept = match std::fs::read(path(node)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("agent-scan.json: {error}")))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Kept::default()),
        Err(error) => return Err(error),
    };
    if kept.version > VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("agent-scan.json is of version {}, newer than this fastctx reads", kept.version),
        ));
    }
    Ok(kept)
}

pub(super) fn write(node: &Node, kept: &Kept) -> io::Result<()> {
    let mut kept = kept.clone();
    kept.version = VERSION;
    let mut bytes = serde_json::to_vec_pretty(&kept).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&path(node), &bytes, Permissions::Private)
}

/// Changes the kept record under the node's lock for it.
pub(super) fn change<T>(node: &Node, edit: impl FnOnce(&mut Kept) -> T) -> io::Result<T> {
    let _keeping = node.logins.keeping.lock().expect("never poisoned");
    let mut kept = read(node)?;
    let result = edit(&mut kept);
    write(node, &kept)?;
    Ok(result)
}

/// The directories the user added, from `agent-homes.json`; what an older
/// World recorded there as hidden is no longer heeded.
#[derive(Default, Serialize, Deserialize)]
struct Added {
    #[serde(default)]
    added: Vec<AddedHome>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct AddedHome {
    pub agent: AgentName,
    pub dir: PathBuf,
}

fn added_path(node: &Node) -> PathBuf {
    node.world.path().join("agent-homes.json")
}

pub(super) fn added(node: &Node) -> io::Result<Vec<AddedHome>> {
    match std::fs::read(added_path(node)) {
        Ok(bytes) => serde_json::from_slice::<Added>(&bytes)
            .map(|added| added.added)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("agent-homes.json: {error}"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Adds `dir` to the directories the user added for `agent`, once.
pub(super) fn add(node: &Node, agent: &AgentName, dir: &Path) -> io::Result<()> {
    let _keeping = node.logins.keeping.lock().expect("never poisoned");
    let mut added = added(node)?;
    if !added.iter().any(|known| known.agent == *agent && crate::homes::same_dir(&known.dir, dir)) {
        added.push(AddedHome { agent: agent.clone(), dir: dir.to_path_buf() });
    }
    let mut bytes = serde_json::to_vec_pretty(&Added { added }).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&added_path(node), &bytes, Permissions::Private)
}
