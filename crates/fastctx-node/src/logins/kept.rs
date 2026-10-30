//! What the node keeps on disk of its agents' logins: the last scan's
//! findings in `<world>/agent-scan.json`, each login's limits as last read
//! with them, and each chosen folder's last search; and what the user chose
//! in `<world>/agent-homes.json`, the logins taken up and the folders
//! searched (`world-agent-logins.md`). Nothing in either is a credential:
//! who is logged in, as the agents said it, and paths.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};
use fastctx_types::AgentName;
use fastctx_types::agents::AgentModel;
use fastctx_types::logins::{FolderSearch, Found, FoundProgram, Identified, Launcher, Scan};
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
    /// Each chosen folder's search, running or the last one.
    #[serde(default)]
    pub folders: Vec<KeptSearch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct KeptSearch {
    pub dir: PathBuf,
    pub search: FolderSearch,
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
    /// Codex's limits as last read or pushed, for each login.
    #[serde(default)]
    pub limits: Vec<KeptLimits>,
    /// Codex login directories the World does not name new conversations
    /// in: naming is on unless turned off there (2026-10-30). Kept apart from
    /// the login list, which a rescan rebuilds.
    #[serde(default)]
    pub untitled: Vec<PathBuf>,
}

/// What a login's agent said when last asked. Which program the login
/// starts is not kept: it is read again each time (`super::program`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct KeptLogin {
    pub dir: PathBuf,
    pub found: Found,
    pub identified: Identified,
    /// The program asked, with the version it gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    /// That program's file as it stood when asked: the version holds while
    /// the file is the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_file: Option<FileStamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AgentModel>,
    /// How the login's sign-in files stood when it was asked
    /// (`super::sign_in`); none in a record kept before they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed: Option<Vec<Option<FileStamp>>>,
}

impl KeptLogin {
    /// The login `dir`, found as `found`, as asking it gave while its
    /// sign-in files stood as `signed`.
    pub fn asked(dir: PathBuf, found: Found, asked: super::identify::Asked, signed: Vec<Option<FileStamp>>) -> Self {
        let asked_file = asked.launcher.as_ref().and_then(|launcher| FileStamp::of(&launcher.path));
        Self {
            dir,
            found,
            identified: asked.identified,
            launcher: asked.launcher,
            asked_file,
            models: asked.models,
            signed: Some(signed),
        }
    }
}

/// A file as it stood: its length and when it was last written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileStamp {
    pub len: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<Timestamp>,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok().and_then(|modified| Timestamp::try_from(modified).ok());
        Some(Self { len: metadata.len(), modified })
    }
}

/// A Codex login's limits as Codex keys them, merged as it pushes them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct KeptLimits {
    pub dir: PathBuf,
    pub at: Timestamp,
    pub buckets: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
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
    let prior = read(node)?;
    for (agent, record) in [("codex", &prior.codex), ("claude", &prior.claude)] {
        for login in &record.logins {
            if crate::usage::accounts::first(node, agent, &login.dir)?.is_none()
                && let fastctx_types::agents::Fact::Known { value } = &login.identified.who
            {
                crate::usage::accounts::select(
                    node,
                    agent,
                    &login.dir,
                    crate::usage::accounts::key(value, &login.signed),
                    login.identified.at,
                )?;
            }
        }
    }
    for limits in &mut kept.codex.limits {
        if limits.account.is_none() {
            limits.account = crate::usage::accounts::first(node, "codex", &limits.dir)?;
        }
    }
    for (agent, record) in [("codex", &kept.codex), ("claude", &kept.claude)] {
        for login in &record.logins {
            if let fastctx_types::agents::Fact::Known { value } = &login.identified.who {
                crate::usage::accounts::select(
                    node,
                    agent,
                    &login.dir,
                    crate::usage::accounts::key(value, &login.signed),
                    login.identified.at,
                )?;
            }
        }
    }
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

/// What the user chose, from `agent-homes.json`: the directories taken up
/// as logins, and the folders searched for logins. What an older World
/// recorded there as hidden is no longer heeded.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Added {
    #[serde(default)]
    pub added: Vec<AddedHome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<PathBuf>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct AddedHome {
    pub agent: AgentName,
    pub dir: PathBuf,
}

fn added_path(node: &Node) -> PathBuf {
    node.world.path().join("agent-homes.json")
}

pub(super) fn chosen(node: &Node) -> io::Result<Added> {
    match std::fs::read(added_path(node)) {
        Ok(bytes) => serde_json::from_slice::<Added>(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("agent-homes.json: {error}"))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Added::default()),
        Err(error) => Err(error),
    }
}

/// Changes what the user chose, under the node's lock for its records.
fn choose(node: &Node, edit: impl FnOnce(&mut Added)) -> io::Result<()> {
    let _keeping = node.logins.keeping.lock().expect("never poisoned");
    let mut added = chosen(node)?;
    edit(&mut added);
    let mut bytes = serde_json::to_vec_pretty(&added).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&added_path(node), &bytes, Permissions::Private)
}

/// Adds `dir` to the directories the user took up for `agent`, once.
pub(super) fn add(node: &Node, agent: &AgentName, dir: &Path) -> io::Result<()> {
    choose(node, |added| {
        if !added.added.iter().any(|known| known.agent == *agent && crate::homes::same_dir(&known.dir, dir)) {
            added.added.push(AddedHome { agent: agent.clone(), dir: dir.to_path_buf() });
        }
    })
}

/// The folders the user chose, searched for logins.
pub(super) fn folders(node: &Node) -> io::Result<Vec<PathBuf>> {
    chosen(node).map(|added| added.folders)
}

/// Adds `dir` to the folders searched for logins, once.
pub(super) fn add_folder(node: &Node, dir: &Path) -> io::Result<()> {
    choose(node, |added| {
        if !added.folders.iter().any(|known| crate::homes::same_dir(known, dir)) {
            added.folders.push(dir.to_path_buf());
        }
    })
}

/// No longer searches `dir` for logins.
pub(super) fn remove_folder(node: &Node, dir: &Path) -> io::Result<()> {
    choose(node, |added| added.folders.retain(|known| !crate::homes::same_dir(known, dir)))
}
