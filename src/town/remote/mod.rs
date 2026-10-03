//! The town across machines (`specs/world-network.md`, "四种话"). A resident's hands reach
//! another machine by call; answers, words for a colleague and the news that a command ended
//! travel by mail; `fastctx w` asks every machine at once; watching a resident who lives
//! elsewhere is a subscription.
//!
//! On every machine the town is the only consumer of the mail and the subscriptions the
//! network brings in: the network carries them, the town knows what they mean.

pub(crate) mod calls;
mod mailroom;
pub(crate) mod verbs;
pub(crate) mod watch;

use super::core::Delivery;
use serde::{Deserialize, Serialize};

/// A resident somewhere in the World: the machine its host runs on, and its id.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Peer {
    pub(crate) machine: String,
    pub(crate) id: String,
}

impl Peer {
    pub(crate) fn new(machine: &str, id: &str) -> Self {
        Self { machine: machine.to_string(), id: id.to_string() }
    }
}

/// What one machine's town mails another's. The machine a letter comes from is the mail's
/// signed sender, so no letter names it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "letter", rename_all = "snake_case")]
pub(crate) enum Letter {
    /// Words or a notice for a resident living on the receiving machine.
    Deliver { to: String, delivery: Delivery },
    /// A resident of the sending machine speaks to a resident of the receiving one; the
    /// answer to the turn its words go into goes back to it.
    Speak { from: String, to: String, message: String },
    /// A resident of the sending machine left; the colleagues it asked for on the receiving
    /// machine sleep once their turn is over.
    Left { requester: String },
}

/// The calls one machine's town makes to another's, each judged by the grant verb beside it:
/// the tool a resident used, or `w` and `talk` for the two things only people and commands do.
const VERBS: [(&str, &str); 9] = [
    ("hand_run", "run"),
    ("hand_edit", "edit"),
    ("hand_write", "write"),
    ("hand_apply_patch", "apply_patch"),
    ("who", "w"),
    ("colleague_ask", "ask_colleague"),
    ("talk_say", "talk"),
    ("talk_open", "talk"),
    ("talk_stop", "talk"),
];

/// The grant verb a town's call is judged by; `None` for a verb that is not a town's.
pub(crate) fn grant_verb(wire: &str) -> Option<&'static str> {
    VERBS.iter().find(|(name, _)| *name == wire).map(|(_, grant)| *grant)
}

/// Whether a town name another machine sent can name a directory here: a town's short name,
/// never a path.
pub(crate) fn valid_town(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
}
