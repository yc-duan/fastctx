//! The kinds of the records the hub keeps of an account and its
//! conversations, in the asset library (design §28.17): the hub's account
//! records are where they are written and merged; as the hub accepts one, it
//! writes the record into a stream it orders — in the conversation's scope
//! for a conversation's plan, notes, goals and sentinels, in the account's
//! scope for its skills and settings — each by its key. A body is the
//! account record itself: its version, who wrote it and when, and what it
//! says. What the hub tells an account goes into its scope the same way: the
//! machine changes it reads (`machine.audit`) and the troubles it hears of
//! (`account.notice`), each a JSON object by its number.

use super::kind::{Actor, Deletion, Holding, Import, Kind};
use super::{Entry, KindRecord};
use crate::State;

fn record(body: &[u8]) -> Result<Vec<u8>, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) if value.get("metadata").is_some_and(serde_json::Value::is_object) => Ok(body.to_vec()),
        Ok(_) => Err("a record of the hub's carries its metadata".into()),
        Err(error) => Err(format!("a record of the hub's is not JSON: {error}")),
    }
}

fn object(body: &[u8]) -> Result<Vec<u8>, String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(value) if value.is_object() => Ok(body.to_vec()),
        Ok(_) => Err("what the hub tells is a JSON object".into()),
        Err(error) => Err(format!("what the hub tells is not JSON: {error}")),
    }
}

/// Whether a body says the record itself is its one object: a record
/// larger than a body holds (AR03).
fn in_object(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("in_object").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn judged(after: &KindRecord, check: fn(&[u8]) -> Result<Vec<u8>, String>) -> Result<(), String> {
    match &after.record.state {
        State::Gone => Ok(()),
        State::Is { body, objects } => {
            check(body)?;
            match (objects.len(), in_object(body)) {
                (0, false) | (1, true) => Ok(()),
                _ => Err("a record of the hub's is its body, or the one object its body says it is in".into()),
            }
        }
    }
}

/// A kind of the hub's records: its name, its number in the registry
/// (`fastctx_sync::registry`, whose order only grows), its stream, how a
/// body is checked and where the hub takes them up from.
macro_rules! hub_record {
    ($(#[$doc:meta])* $name:ident, $number:literal, $stream:literal) => {
        hub_record!($(#[$doc])* $name, $number, $stream, record, "account-records");
    };
    ($(#[$doc:meta])* $name:ident, $number:literal, $stream:literal, $check:ident, $import:literal) => {
        $(#[$doc])*
        pub struct $name;

        impl Kind for $name {
            const KIND: u16 = $number;
            const SCHEMA: u16 = 1;
            const STREAM: &'static str = $stream;
            const HOLDING: Holding = Holding::Readers;
            /// A record larger than a body holds is its one object (AR03).
            const ROLES: &'static [&'static str] = &["record"];
            const RETENTION: Option<u64> = None;
            const DELETION: Deletion = Deletion::Tombstone;
            /// Taken up from where the hub keeps them.
            const IMPORT: Import = Import::From($import);
            type Value = Vec<u8>;

            fn encode(value: &Vec<u8>) -> Vec<u8> {
                value.clone()
            }

            fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
                $check(body)
            }

            /// The hub alone orders the stream (`HUB_ORDERED`): what is
            /// judged is that each record is as its kind's are.
            fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
                judged(after, $check)
            }

            fn subscription(scope: &str, _stream: &str) -> String {
                format!("{}:{scope}", $stream)
            }
        }
    };
}

hub_record!(
    /// A conversation's plan (`conversation.plan`).
    ConversationPlan, 12, "plan"
);
hub_record!(
    /// A conversation's notes, one item each (`conversation.history`).
    ConversationHistory, 13, "history"
);
hub_record!(
    /// A conversation's goals, the one it pursues and those past
    /// (`conversation.goal`).
    ConversationGoal, 14, "goals"
);
hub_record!(
    /// A conversation's sentinels as registered (`conversation.sentinel`).
    ConversationSentinel, 15, "sentinels"
);
hub_record!(
    /// An account's settings kept as records (`account.settings`).
    AccountSettings, 4, "settings"
);
hub_record!(
    /// An account's skills and their index (`account.skill`).
    AccountSkill, 5, "skills"
);
hub_record!(
    /// The machine changes an account reads, as the hub logged them, by
    /// their number (`machine.audit`).
    MachineAudit, 30, "audit", object, "machine-log"
);
hub_record!(
    /// The troubles an account hears of, by their number, as raised and
    /// as resolved (`account.notice`).
    AccountNotice, 27, "notices", object, "troubles"
);
hub_record!(
    /// An account's own layer of configuration — its MCP servers, the values
    /// their addresses are asked with among them — as the hub keeps it
    /// (`account.profile`).
    AccountProfile, 2, "profile", object, "directory"
);
hub_record!(
    /// How an account's models work together, as the hub keeps it
    /// (`account.collaboration`).
    AccountCollaboration, 3, "collaboration", object, "directory"
);

/// A project's ledger (`project.ledger`, design §28.18): each entry the hub
/// numbered, by its number, its signed bytes as they were handed — the
/// author's signature with them — the one object; the body says its number
/// and length. The project's notes, pages and record are files of these
/// entries.
pub struct ProjectLedger;

impl Kind for ProjectLedger {
    const KIND: u16 = 22;
    const SCHEMA: u16 = 1;
    const STREAM: &'static str = "ledger";
    const HOLDING: Holding = Holding::Readers;
    const ROLES: &'static [&'static str] = &["entry"];
    const RETENTION: Option<u64> = None;
    const DELETION: Deletion = Deletion::Tombstone;
    /// Taken up from the hub's ledgers.
    const IMPORT: Import = Import::From("ledger");
    type Value = Vec<u8>;

    fn encode(value: &Vec<u8>) -> Vec<u8> {
        value.clone()
    }

    fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
        let value: serde_json::Value =
            serde_json::from_slice(body).map_err(|error| format!("a ledger entry's record: {error}"))?;
        value.get("seq").and_then(serde_json::Value::as_u64).ok_or("a ledger entry's record says its number")?;
        Ok(body.to_vec())
    }

    /// The hub alone orders the stream: each entry is its one object, as
    /// long as its record says.
    fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
        match &after.record.state {
            State::Gone => Ok(()),
            State::Is { body, objects } => {
                let value: serde_json::Value =
                    serde_json::from_slice(body).map_err(|error| format!("a ledger entry's record: {error}"))?;
                let size = value.get("size").and_then(serde_json::Value::as_u64);
                match objects.as_slice() {
                    [entry] if Some(entry.size) == size => Ok(()),
                    _ => Err("a ledger entry's record refers to its entry, as long as it says".into()),
                }
            }
        }
    }

    fn subscription(scope: &str, _stream: &str) -> String {
        format!("ledger:{scope}")
    }
}
