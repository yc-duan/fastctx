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
    /// The assistance it was born working by, as its host said and the hub
    /// found it gave the account on that machine; kept once, never changed
    /// (design-forget-account-v2.md F0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub born_under: Option<crate::access::GrantId>,
    /// Which account of its name it was born working for: when that account
    /// was opened, as its host said and the hub found it; kept once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<jiff::Timestamp>,
    /// A colleague a conversation asked for, as its host said; kept once.
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
        /// account registering it, and whether it is a colleague. Older
        /// hosts omit them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        born_under: Option<crate::access::GrantId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation: Option<jiff::Timestamp>,
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
