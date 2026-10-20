//! User actions on a resident's membership, presence and project.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::TownName;
use crate::residents::{Activity, Landed};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum Management {
    /// Takes the conversation out of the list into the archived ones,
    /// stopping a turn it runs first; nothing of it is deleted
    /// (`world-colleagues.md`, 用户在任何地方和居民说话). Written under the
    /// name it had before archiving was its own, so nodes of every version
    /// read it: theirs removed it the same way.
    #[serde(rename = "remove")]
    Archive,
    /// Brings an archived conversation back where it was, to be spoken to
    /// again.
    Restore,
    Sleep,
    Wake,
    /// None leaves every project.
    Move {
        town: Option<TownName>,
    },
    /// The machines and folders the user points it to, replacing those
    /// before (`world-gui.md`, 对话).
    Focus {
        focus: crate::focus::Focus,
    },
    /// The account's MCP servers it goes without, replacing those before
    /// (`world-user-config.md`); it takes the others at its next start.
    Mcp {
        off: std::collections::BTreeSet<String>,
    },
}

/// Each completed stage remains reported if a later stage fails. A host
/// refusal never implies that the World closed a user's own window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Managed {
    /// Membership is removed even if the host step failed. Activity below
    /// remains the last observation of the host, not a membership flag.
    pub removed: Option<Timestamp>,
    pub town: Option<TownName>,
    pub activity: Option<Activity>,
    pub steps: Vec<ManagementStep>,
    pub notification: Option<Landed>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementStep {
    /// `host`, `records`, `tools`, or `notification`.
    pub stage: String,
    pub outcome: ManagementOutcome,
    pub detail: String,
}

/// How deleting a conversation everywhere went, as far as the device that
/// asked can say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Forgetting {
    /// It lived on this computer, in no network: it is deleted.
    Deleted,
    /// Its hub took the deletion: each machine and device keeping something
    /// of it deletes that when it is connected, now or when it comes back.
    Asked,
    /// This computer is not connected to its hub now: it asks the hub when
    /// it is, having deleted what it kept itself.
    Queued,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagementOutcome {
    Done,
    Pending,
    Unsupported,
    Failed,
}
