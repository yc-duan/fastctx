//! Names in the World.
//!
//! Each kind of name has one type and one rule, checked where the name enters
//! the program (parsing an argument, reading a record, receiving a message),
//! so code that holds a name never checks it again. Names are plain words or
//! short typed ids, never `scheme://` forms (`world.md`, 句柄).

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
#[error("{value:?} is not a valid {kind}: {rule}")]
pub struct NameError {
    pub kind: &'static str,
    pub value: String,
    pub rule: &'static str,
}

/// Lowercase ASCII letters, digits and hyphens, starting with a letter, not
/// ending with a hyphen, at most `max` bytes. Such a word is typed in
/// commands on every keyboard, passes through every shell unquoted, and is
/// a valid file name on every platform the World runs on.
fn is_word(text: &str, max: usize) -> bool {
    !text.is_empty()
        && text.len() <= max
        && text.as_bytes()[0].is_ascii_lowercase()
        && !text.ends_with('-')
        && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

macro_rules! name_type {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $rule:literal, $check:expr) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = NameError;

            fn try_from(value: String) -> Result<Self, NameError> {
                let check: fn(&str) -> bool = $check;
                if check(&value) {
                    Ok(Self(value))
                } else {
                    Err(NameError { kind: $kind, value, rule: $rule })
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = NameError;

            fn try_from(value: &str) -> Result<Self, NameError> {
                Self::try_from(value.to_string())
            }
        }

        impl std::str::FromStr for $name {
            type Err = NameError;

            fn from_str(value: &str) -> Result<Self, NameError> {
                Self::try_from(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }

        impl Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }
    };
}

/// The name the hub signs its own messages with; no machine may take it.
pub const HUB: &str = "hub";

name_type!(
    /// A machine in the World, such as `desktop` or `laptop`: how a resident
    /// says where something happens, and how every response says where it did.
    MachineName,
    "machine name",
    "a word of lowercase letters, digits and hyphens, starting with a letter, at most 32 characters, and not `hub`",
    |text| is_word(text, 32) && text != HUB
);

name_type!(
    /// A town's short name, such as `mark50`: what commands, `fastctx w` and
    /// the World's directories use; the project's full name is free text.
    TownName,
    "town name",
    "a word of lowercase letters, digits and hyphens, starting with a letter, at most 32 characters",
    |text| is_word(text, 32)
);

name_type!(
    /// The kind of agent a resident is, such as `codex` or `claude`; the
    /// first half of its id. The user is a resident too, as `user`.
    AgentName,
    "agent name",
    "a word of lowercase letters, digits and hyphens, starting with a letter, at most 16 characters",
    |text| is_word(text, 16)
);

name_type!(
    /// A resident, such as `codex-7f3a`: the agent name, a hyphen, and at
    /// least four lowercase hex digits. Unique on the machine the resident
    /// lives on; across machines a resident is its machine plus this id.
    ResidentId,
    "resident id",
    "an agent name, a hyphen, and four or more lowercase hex digits",
    |text| match text.rsplit_once('-') {
        Some((agent, digits)) => {
            is_word(agent, 16)
                && digits.len() >= 4
                && digits.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
);

name_type!(
    /// Something a resident started running, such as `j-k3x9a2`.
    JobId,
    "job id",
    "`j-` followed by six lowercase letters or digits",
    |text| text.len() == 8
        && text.starts_with("j-")
        && text[2..].bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
);

name_type!(
    /// A journal entry, such as `20260701T031205Z-laptop`: the UTC second it
    /// was written and the machine it was written on. Stable everywhere, so
    /// entries are cited by id, never by position.
    EntryId,
    "journal entry id",
    "a UTC time as YYYYMMDDTHHMMSSZ, a hyphen, and a machine name",
    |text| match text.split_once('-') {
        Some((time, machine)) => is_entry_time(time) && MachineName::try_from(machine).is_ok(),
        None => false,
    }
);

fn is_entry_time(time: &str) -> bool {
    let bytes = time.as_bytes();
    bytes.len() == 16
        && bytes[8] == b'T'
        && bytes[15] == b'Z'
        && bytes[..8].iter().chain(&bytes[9..15]).all(u8::is_ascii_digit)
}

impl ResidentId {
    pub fn agent(&self) -> AgentName {
        let (agent, _) = self.0.rsplit_once('-').expect("checked on entry");
        AgentName(agent.to_string())
    }
}

/// Where a resident lives: its machine and its id there.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Address {
    pub machine: MachineName,
    pub resident: ResidentId,
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on {}", self.resident, self.machine)
    }
}
