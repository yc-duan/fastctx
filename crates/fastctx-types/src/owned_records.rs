//! Conversation identity and access to its records without impersonating its opener.

use crate::{AccountName, Address, MachineName, ResidentId, account_records};
use serde::{Deserialize, Serialize};

pub const KIND: &str = "conversation";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub account: AccountName,
    pub address: Address,
    /// An assisted birth is never opened to the host's whole-machine holders.
    pub shared_on: Option<MachineName>,
    /// The machine it was born on as it was then: its key and the account it
    /// was registered under. A machine that later takes the name, or the same
    /// machine under another owner, holds none of its records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub born_seal: Option<String>,
    /// Previous runtime addresses remain aliases after a manual conversation move.
    #[serde(default)]
    pub previous: Vec<MachineName>,
    /// The canonical conversation, mapped once by the hub from the birth facts
    /// its host reported. Absent for a conversation registered without them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Box<crate::sync::identity::ObjectId>>,
    /// What it was born as, for its account (design-forget-account-v2.md
    /// F0): kept once, whole, as its host says it and the hub finds it
    /// holds, never changed after but for an assistance still to be found
    /// becoming found. Written among the conversation's own fields.
    #[serde(flatten)]
    pub birth: Box<Birth>,
}

/// What a conversation was born as, for its account (`Conversation::birth`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Birth {
    /// Which account of its name it was born working for: when that account
    /// was opened. None where nothing was said: born before this was kept, on
    /// a host from before it, for those logged in on its computer, or not
    /// born whole yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<jiff::Timestamp>,
    /// It was born working by an assistance of the login, the account not
    /// holding the machine.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assisted: bool,
    /// The assistance it was born working by, as the hub found it gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub born_under: Option<crate::access::GrantId>,
    /// The assistance its host said it was born working by, which the hub
    /// could not find it gave, as one ended before the conversation was
    /// registered: still to be found, never taken for found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_under: Option<crate::access::GrantId>,
    /// A colleague a conversation asked for, as its host said.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub colleague: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    /// A host registers only a conversation it runs for the authenticated account.
    Register {
        resident: ResidentId,
        shared: bool,
        #[serde(default)]
        previous: Option<MachineName>,
        /// The original birth time from the host's resident record, from which
        /// the hub maps the canonical conversation. Older hosts omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        born_at: Option<jiff::Timestamp>,
        /// What the host recorded as it was born (`About::born_as`) of the
        /// account registering it, said once it was born whole, and whether
        /// it is a colleague. Older hosts omit them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation: Option<jiff::Timestamp>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        assisted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        born_under: Option<crate::access::GrantId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        colleague: bool,
    },
    Delete {
        resident: ResidentId,
    },
    /// The runtime host durably records deletion recipients before removing ownership.
    PrepareDeletion {
        conversation: Address,
    },
    Authorize {
        conversation: Address,
        reader: AccountName,
    },
    PlanHead {
        resident: ResidentId,
        conversation: Option<Address>,
    },
    Sentinels,
    List,
    Replicas,
    Submitted {
        conversation: Address,
        kind: String,
        key: String,
        submission: String,
    },
    Catalog {
        conversation: Address,
    },
    Resolve {
        conversation: Address,
    },
    Record {
        conversation: Address,
        request: account_records::Request,
    },
    Revision {
        conversation: Address,
        kind: String,
        key: String,
        version: u64,
    },
    Versions {
        conversation: Address,
        kind: String,
        key: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    PlanHead { conversation: Option<Box<Conversation>>, metadata: Option<account_records::Metadata> },
    Authorization { allowed: bool },
    Sentinels { registrations: Vec<crate::sentinel_control::Registration> },
    Deleted,
    DeletionPrepared,
    Conversations { conversations: Vec<Conversation> },
    Replicas { catalog: crate::replicas::Catalog },
    Submitted { record: Option<account_records::Record> },
    Conversation { conversation: Box<Conversation> },
    Record { response: account_records::Response },
    Revision { record: account_records::Record },
    Versions { versions: Vec<Version> },
    Catalog { catalog: crate::history::Catalog },
    /// The conversation's machine registered no conversation: one from
    /// before the records, whose conversations' records are their
    /// account's own, read and changed as before them.
    Unregistered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub metadata: account_records::Metadata,
    pub history: Option<crate::history::HistoryRevision>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_conversation_was_born_as_is_written_among_its_own_fields() {
        let mut conversation: Conversation = serde_json::from_value(serde_json::json!({
            "account": "carol", "address": {"machine": "host", "resident": "codex-ab12"}, "shared_on": null,
        }))
        .expect("one kept before births were");
        assert_eq!(*conversation.birth, Birth::default());
        assert_eq!(
            serde_json::to_value(&conversation).expect("written"),
            serde_json::json!({"account": "carol", "address": {"machine": "host", "resident": "codex-ab12"}, "shared_on": null, "previous": []})
        );
        conversation.birth.generation = Some("2026-11-01T00:00:00Z".parse().expect("a time"));
        conversation.birth.assisted = true;
        conversation.birth.claimed_under = Some(crate::access::GrantId::from_bytes([7; 8]));
        let written = serde_json::to_value(&conversation).expect("written");
        assert_eq!(written["generation"], "2026-11-01T00:00:00Z");
        assert_eq!(written["assisted"], true);
        assert_eq!(written["claimed_under"], "g-0707070707070707");
        assert!(written.get("birth").is_none() && written.get("born_under").is_none());
        assert_eq!(serde_json::from_value::<Conversation>(written).expect("read again"), conversation);
    }
}
