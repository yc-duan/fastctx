//! A conversation's actual scratch directory on a machine, reported by that
//! machine rather than inferred from its user home or installation defaults.

use crate::{MachineName, access::HubId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// None identifies the machine answering this particular request. A relay
    /// binds it to its authenticated hub before passing the location farther.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubId>,
    pub machine: MachineName,
    pub path: Result<String, String>,
}

pub type Descriptions = std::collections::BTreeMap<String, Result<String, String>>;

pub mod words {
    pub const OLD_HUB: &str = "update the hub to obtain conversation scratch paths";
    pub const OLD_NODE: &str = "update that node to obtain conversation scratch paths";
    pub const WRONG_REPLY: &str = "the machine did not return a scratch path";
    pub const UNREPORTED: &str = "this route did not report scratch paths; update its nodes and hub";
    pub const OFFLINE: &str = "the machine is offline";
    pub fn note(machine: &str, path: &Result<String, String>) -> String {
        match path {
            Ok(path) => format!("=== Scratch on {machine}: {path} ==="),
            Err(error) => format!("=== Scratch on {machine} is unavailable: {error} ==="),
        }
    }
    pub fn described(machine: &str, path: &Result<String, String>) -> String {
        match path {
            Ok(path) => format!("{machine} (scratch: {path})"),
            Err(error) => format!("{machine} (scratch unavailable: {error})"),
        }
    }
}
