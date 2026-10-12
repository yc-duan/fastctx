//! What a conversation is about, as the user points it out: machines, and
//! folders on them (`world-gui.md`, 新对话). It is said to the model as a
//! hint and never limits anything: the machines a conversation may use are
//! those of the account it works for.

use serde::{Deserialize, Serialize};

use crate::MachineName;
use crate::access::Written;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Focus {
    /// In the order the user chose them.
    #[serde(default)]
    pub machines: Vec<FocusedMachine>,
}

impl Focus {
    pub fn is_empty(&self) -> bool {
        self.machines.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusedMachine {
    pub machine: Written,
    /// Folders on it, each an absolute path as that machine writes it
    /// (`C:/Users/me` on Windows); none points to the machine as a whole.
    #[serde(default)]
    pub folders: Vec<String>,
}

/// The folders in one folder of a machine, for choosing one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folders {
    pub machine: MachineName,
    /// The folder listed; `None` for the places a choice starts from.
    pub path: Option<String>,
    /// The folder holding it, where it has one; `None` at a root, where the
    /// way up is back to the places a choice starts from.
    pub parent: Option<String>,
    /// The user's home there, which a choice usually starts from.
    pub home: String,
    pub folders: Vec<Folder>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// Its name, or for a place a choice starts from, its whole path.
    pub name: String,
    pub path: String,
    /// Its name begins with a dot, or the system marks it hidden.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// What a folder holds, as the model is told it, or why it could not be
/// read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outlined {
    pub path: String,
    pub outline: Result<String, String>,
}
