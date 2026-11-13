//! The kinds of the records the hub keeps of an account and its
//! conversations, in the asset library (design §28.17): the hub's account
//! records are where they are written and merged; as the hub accepts one, it
//! writes the record into a stream it orders — in the conversation's scope
//! for a conversation's plan, notes, goals and sentinels, in the account's
//! scope for its skills and settings — each by its key. A body is the
//! account record itself: its version, who wrote it and when, and what it
//! says.

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

fn judged(after: &KindRecord) -> Result<(), String> {
    match &after.record.state {
        State::Gone => Ok(()),
        State::Is { body, objects } if objects.is_empty() => record(body).map(|_| ()),
        State::Is { .. } => Err("a record of the hub's refers to no object".into()),
    }
}

/// A kind of the hub's records: its name, its number in the registry
/// (`fastctx_sync::registry`, whose order only grows) and its stream.
macro_rules! hub_record {
    ($(#[$doc:meta])* $name:ident, $number:literal, $stream:literal) => {
        $(#[$doc])*
        pub struct $name;

        impl Kind for $name {
            const KIND: u16 = $number;
            const SCHEMA: u16 = 1;
            const STREAM: &'static str = $stream;
            const HOLDING: Holding = Holding::Readers;
            const ROLES: &'static [&'static str] = &[];
            const RETENTION: Option<u64> = None;
            const DELETION: Deletion = Deletion::Tombstone;
            /// Taken up from the hub's account records.
            const IMPORT: Import = Import::From("account-records");
            type Value = Vec<u8>;

            fn encode(value: &Vec<u8>) -> Vec<u8> {
                value.clone()
            }

            fn decode(body: &[u8]) -> Result<Vec<u8>, String> {
                record(body)
            }

            /// The hub alone orders the stream (`HUB_ORDERED`): what is
            /// judged is that each record is an account record.
            fn authorize(_actor: &Actor<'_>, _before: Option<&Entry>, after: &KindRecord) -> Result<(), String> {
                judged(after)
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
