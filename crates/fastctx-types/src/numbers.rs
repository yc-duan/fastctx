//! What a machine's answers to others stand at, kind by kind
//! (`interaction-design`, 变化编号): told its hub with each heartbeat, and by
//! the hub beside its presence to each reader that may ask it for that kind.
//! A reader holding such an answer asks again once the number moved, and
//! not before; one that never held one asks when it needs it.

use serde::{Deserialize, Serialize};

/// The numbers a machine's answers stand at. Each moves whenever what the
/// machine answers of its kind changes; a number that did not move says its
/// answer is the same. None of a kind from a machine that does not tell it,
/// or to a reader that may not ask for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Numbers {
    /// Its agents, logins and their usage (`Call::Agents`, `AgentLogins`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logins: Option<u64>,
    /// Its book of what was done there (`RemoteCall::Book`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book: Option<u64>,
    /// How its updates stand (`UpdateCall::State` asked of it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<u64>,
    /// How the sentinels it is home to stand (`HomeRequest::Inspect`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinels: Option<u64>,
}

impl Numbers {
    /// Whether it tells nothing.
    pub fn is_empty(&self) -> bool {
        self.logins.is_none() && self.book.is_none() && self.updates.is_none() && self.sentinels.is_none()
    }
}
