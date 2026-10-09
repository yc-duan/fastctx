//! `fastctx copy`: files and directories from one machine to another. The
//! bytes go over a data connection between the two machines, through the
//! hub, and never through a resident's context (`world.md`, 数据不过上下文).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;

/// One end of a copy: a path on a machine; `None` is the machine asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub machine: Option<MachineName>,
    /// The hub of `machine`, when it was written as `name.hub`
    /// (`world-network.md`, 机器的名字).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<crate::HubName>,
    pub path: String,
}

impl Place {
    /// The machine as it was written, if one was.
    pub fn written(&self) -> Option<crate::access::Written> {
        self.machine.clone().map(|name| crate::access::Written { name, hub: self.hub.clone() })
    }
}

/// Everything a copy carries, as its source found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// What was copied: a single file, or a directory and what is in it.
    pub directory: bool,
    /// The last name of the source path, for a copy into a directory.
    pub name: String,
    /// The root first, then each directory before what is in it.
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative to the root, names joined by `/`; empty for the root.
    pub path: String,
    pub kind: EntryKind,
    /// Unix permission bits, when the source has them.
    pub mode: Option<u32>,
    pub modified: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EntryKind {
    File {
        size: u64,
        sha256: String,
    },
    Directory,
    /// A symbolic link, copied as a link: what it points to is not followed.
    Symlink {
        target: String,
    },
}

/// What a copy did, as the destination saw it, with what the source could
/// not read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyReport {
    pub from: String,
    pub from_machine: MachineName,
    /// The path the copy made or filled.
    pub to: String,
    pub to_machine: MachineName,
    pub files: u64,
    pub bytes: u64,
    pub directories: u64,
    pub links: u64,
    /// Files that were there already with the same contents.
    pub unchanged: u64,
    /// What was not copied, and why.
    pub problems: Vec<String>,
}
