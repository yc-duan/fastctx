//! Waiting for a machine's state to change, in place of asking for it again
//! and again. What a node keeps and shows is named by topic, and each topic
//! counts the times it changed since the node started. A reader asks where
//! the topics it reads from stand, reads what they name, and then asks to be
//! told when any of them is past the count it read at: it reads again only
//! then. What it reads, and by which calls, is as before.

use serde::{Deserialize, Serialize};

/// The most topics one asking names.
pub const TOPICS: usize = 64;
/// The longest topic name, in bytes.
pub const TOPIC_BYTES: usize = 128;

/// The logins of agents on the machine and what is known of each
/// (`Call::AgentLogins`, `Call::Agents`).
pub const LOGINS: &str = "logins";

/// A topic and the times it had changed, in one run of the node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicAt {
    pub topic: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicsAsk {
    /// The run of the node the revisions were told in; none from a reader
    /// that was told none yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The topics read from, each at the revision the reader read it at.
    pub known: Vec<TopicAt>,
}

impl TopicsAsk {
    pub fn validate(&self) -> Result<(), String> {
        if self.known.is_empty() || self.known.len() > TOPICS {
            return Err(format!("name 1 to {TOPICS} topics to wait on"));
        }
        if self.known.iter().any(|known| known.topic.is_empty() || known.topic.len() > TOPIC_BYTES) {
            return Err(format!("a topic is named in 1 to {TOPIC_BYTES} bytes"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicsTold {
    /// This run of the node. Revisions told in another run compare with
    /// nothing here.
    pub run: String,
    /// The topics past where the reader read them, each where it stands
    /// now. Every topic asked, when the reader named no run or another
    /// one: it reads all of them. None when nothing changed while the
    /// asking waited: the reader asks again.
    pub changed: Vec<TopicAt>,
}
