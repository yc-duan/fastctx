//! Names in the World.
//!
//! Each kind of name has one type and one rule, checked where the name enters
//! the program (parsing an argument, reading a record, receiving a message),
//! so code that holds a name never checks it again. Names are plain words or
//! short typed ids, never `scheme://` forms (`world.md`, 句柄).

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use serde::{Deserialize, Serialize};

mod spelled;

pub(crate) use spelled::nfkc;
pub use spelled::{MAX_BYTES as NAME_MAX_BYTES, MAX_CHARS as NAME_MAX_CHARS, NameProblem};

#[derive(Debug, thiserror::Error)]
#[error("{value:?} is not a valid {kind}: {rule}")]
pub struct NameError {
    pub kind: &'static str,
    pub value: String,
    pub rule: Cow<'static, str>,
    /// What is wrong, for a machine or a hub name.
    pub problem: Option<NameProblem>,
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
    // A name kept in one canonical spelling: `$canonical` gives it, or
    // `None` when the text is not such a name.
    ($(#[$meta:meta])* $name:ident, $kind:literal, $rule:literal, canonical = $canonical:expr) => {
        name_type!(@type $(#[$meta])* $name);

        impl TryFrom<String> for $name {
            type Error = NameError;

            fn try_from(value: String) -> Result<Self, NameError> {
                let canonical: fn(&str) -> Option<String> = $canonical;
                match canonical(&value) {
                    Some(text) => Ok(Self(text)),
                    None => Err(NameError { kind: $kind, value, rule: Cow::Borrowed($rule), problem: None }),
                }
            }
        }

        name_type!(@rest $name);
    };
    ($(#[$meta:meta])* $name:ident, $kind:literal, $rule:literal, $check:expr) => {
        name_type!(@type $(#[$meta])* $name);

        impl TryFrom<String> for $name {
            type Error = NameError;

            fn try_from(value: String) -> Result<Self, NameError> {
                let check: fn(&str) -> bool = $check;
                if check(&value) {
                    Ok(Self(value))
                } else {
                    Err(NameError { kind: $kind, value, rule: Cow::Borrowed($rule), problem: None })
                }
            }
        }

        name_type!(@rest $name);
    };
    (@type $(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
    (@rest $name:ident) => {
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

/// A name spelled by `spelled`: kept as written, compared, hashed and
/// ordered by its key, so `GPU-2`, `ＧＰＵ-2` and `gpu-2` are one name.
macro_rules! spelled_name_type {
    ($(#[$meta:meta])* $name:ident, $kind:literal, $reserved:expr) => {
        $(#[$meta])*
        // The written form, then the key where it differs, in one string:
        // as small as the plain `String` such names were, since they sit in
        // nearly every record and message.
        #[derive(Clone, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name {
            text: Box<str>,
            written: u32,
        }

        impl $name {
            fn new(written: String, key: String) -> Self {
                let length = u32::try_from(written.len()).expect("a name is at most 64 bytes");
                let text = if key == written { written } else { written + &key };
                Self { text: text.into_boxed_str(), written: length }
            }

            /// The name as written: what is shown, sent and kept.
            pub fn as_str(&self) -> &str {
                &self.text[..self.written as usize]
            }

            /// What the name is compared by: its NFKC case fold. Hash it,
            /// derive ids from it, and match it against other names by it.
            pub fn key(&self) -> &str {
                match &self.text[self.written as usize..] {
                    "" => &self.text,
                    key => key,
                }
            }

            /// The name as one component of a file name, the same on every
            /// system for every spelling of the name. A component that holds
            /// more than the name (`codex-7f3a@desktop`) takes `key` instead:
            /// only a whole component can be a Windows device name.
            pub fn file_name(&self) -> Cow<'_, str> {
                spelled::file_name(self.key())
            }

            /// Whether releases from before names of any language read the
            /// name: lowercase English letters, digits and hyphens, starting
            /// with a letter, at most 32 of them.
            pub fn is_ascii_word(&self) -> bool {
                is_word(self.as_str(), 32)
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.key() == other.key()
            }
        }

        impl Eq for $name {}

        impl Hash for $name {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.key().hash(state)
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for $name {
            fn cmp(&self, other: &Self) -> Ordering {
                self.key().cmp(other.key())
            }
        }

        impl TryFrom<String> for $name {
            type Error = NameError;

            fn try_from(value: String) -> Result<Self, NameError> {
                let reserved: fn(&str) -> bool = $reserved;
                let problem = match spelled::spell(&value) {
                    Ok(spelled) if !reserved(&spelled.key) => {
                        return Ok(Self::new(spelled.written, spelled.key));
                    }
                    Ok(_) => NameProblem::Reserved,
                    Err(problem) => problem,
                };
                Err(NameError { kind: $kind, value, rule: Cow::Owned(problem.to_string()), problem: Some(problem) })
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
                value.as_str().to_string()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(self.as_str(), f)
            }
        }
    };
}

spelled_name_type!(
    /// A machine in the World, such as `desktop`, `laptop` or `台式机`: how a
    /// resident says where something happens, and how every response says
    /// where it did. Unique on its hub by key; never `hub`.
    MachineName,
    "machine name",
    |key| key == HUB
);

impl MachineName {
    /// `parent`'s entry for this machine, named `file_name`. Before that, a
    /// machine whose name is a Windows device name had its entry under the
    /// name itself, which Linux and Windows 11 allow; such a directory is
    /// moved to where it is kept now the first time it is wanted.
    pub fn entry_in(&self, parent: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        let path = parent.join(&*self.file_name());
        if *self.file_name() != *self.key() {
            let before = parent.join(self.key());
            if std::fs::symlink_metadata(&before).is_ok_and(|kept| kept.is_dir()) && !std::fs::exists(&path)? {
                std::fs::rename(&before, &path)?;
            }
        }
        Ok(path)
    }

    /// The name a computer called `host` takes until it is given one: the
    /// host name's first label, lowercase, every run of characters a name
    /// does not take turned into one hyphen, cut to the length a name
    /// allows; `machine` where that leaves no name.
    pub fn from_host(host: &str) -> MachineName {
        let label = host.split('.').next().unwrap_or(host);
        let mut word = String::new();
        for c in label.chars().flat_map(char::to_lowercase) {
            if c.is_alphanumeric() {
                word.push(c);
            } else if !word.is_empty() && !word.ends_with('-') {
                word.push('-');
            }
        }
        loop {
            match MachineName::try_from(word.trim_end_matches('-')) {
                Ok(name) => return name,
                Err(NameError { problem: Some(NameProblem::TooLong), .. }) => {
                    word.pop();
                }
                Err(_) => return MachineName::try_from("machine").expect("`machine` is a machine name"),
            }
        }
    }
}

spelled_name_type!(
    /// A hub's short name, such as `home` or `acme`, chosen by its
    /// administrator: what tells two hubs apart on a computer that uses both,
    /// written after a machine's name as `gpu.acme`.
    HubName,
    "hub name",
    |_| false
);

name_type!(
    /// An account on a hub, such as `alice` or `alice-work`: what a person,
    /// several people or one of a person's roles logs in as. It belongs to
    /// one hub; the same name on another hub is another account.
    AccountName,
    "account name",
    "a word of lowercase letters, digits and hyphens, starting with a letter, at most 64 characters",
    |text| is_word(text, 64)
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
    /// A journal's subject, such as `webrtc-weak-network`: lowercase English
    /// words joined by hyphens, naming a thing, never a conclusion.
    JournalName,
    "journal name",
    "lowercase words of letters and digits joined by hyphens, starting with a letter, at most 64 characters",
    |text| is_word(text, 64) && !text.contains("--")
);

name_type!(
    /// A page's name, with the same spelling rules as a journal's.
    PageName,
    "page name",
    "lowercase words of letters and digits joined by hyphens, starting with a letter, at most 64 characters",
    |text| is_word(text, 64) && !text.contains("--")
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
    /// A sentinel registered by a conversation or a goal.
    SentinelId,
    "sentinel id",
    "`s-` followed by sixteen lowercase hex digits",
    |text| text.len() == 18 && text.starts_with("s-")
        && text[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    canonical = |text| match text.split_once('-') {
        // The machine by its key, so every spelling of it cites one entry.
        Some((time, machine)) if is_entry_time(time) => {
            MachineName::try_from(machine).ok().map(|machine| format!("{time}-{}", machine.key()))
        }
        _ => None,
    }
);

name_type!(
    /// Something told to a resident: its file name in the inbox and the id
    /// of the letter that carries it, such as `desktop-j-k3x9a2`. Lowercase
    /// letters, digits and hyphens.
    NoticeId,
    "notice id",
    "lowercase letters, digits and hyphens, at most 64 characters",
    |text| !text.is_empty()
        && text.len() <= 64
        && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
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

impl Address {
    /// The address as one word, `codex-7f3a@desktop`: how it is written in
    /// environment variables and shown.
    pub fn tag(&self) -> String {
        format!("{}@{}", self.resident, self.machine)
    }

    /// The address as one word with its machine's key: the same for every
    /// spelling of the machine, for directory names on other machines and
    /// for ids made from the address.
    pub fn key(&self) -> String {
        format!("{}@{}", self.resident, self.machine.key())
    }
}

impl std::str::FromStr for Address {
    type Err = NameError;

    fn from_str(text: &str) -> Result<Self, NameError> {
        let invalid = || NameError {
            kind: "resident address",
            value: text.to_string(),
            rule: Cow::Borrowed("a resident id, `@`, and a machine name"),
            problem: None,
        };
        let (resident, machine) = text.split_once('@').ok_or_else(invalid)?;
        Ok(Self {
            machine: MachineName::try_from(machine).map_err(|_| invalid())?,
            resident: ResidentId::try_from(resident).map_err(|_| invalid())?,
        })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on {}", self.resident, self.machine)
    }
}
