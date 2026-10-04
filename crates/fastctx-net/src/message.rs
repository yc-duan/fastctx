//! What members say to each other through the hub, each message signed by
//! the machine that says it.

use fastctx_types::MachineName;
use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use serde::{Deserialize, Serialize};

use crate::trust::WorldId;
use crate::wire::Carried;

/// A message from one member to another. `n` counts up across everything
/// the sender ever signs and is never reused, so the receiver can tell a
/// call it has already answered from a new one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub world: WorldId,
    pub from: MachineName,
    pub to: MachineName,
    pub n: u64,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Body {
    /// Asks the receiver to do something now.
    Call { call: RemoteCall },
    /// A call too large for a message: it follows over a data connection,
    /// as the JSON of a `RemoteCall`.
    LargeCall { carried: Carried },
    /// A call that comes with a data connection, `ticket`, whose source is
    /// the caller: the two ends exchange what the call is about, such as
    /// the files of a copy, while the call waits for its answer.
    StreamCall { call: RemoteCall, ticket: crate::sign::Signed },
    /// The answer to the sender's call numbered `call`.
    Answer { call: u64, result: RemoteResult },
    /// The answer to the call numbered `call`, too large for a message: it
    /// follows over a data connection, as the JSON of a `RemoteResult`.
    LargeAnswer { call: u64, carried: Carried },
    /// A letter, which the sender names with `id`: the receiver takes a
    /// letter from one sender with one id once, and refuses the same id
    /// with different contents.
    Letter { id: LetterId, letter: Letter },
}

/// A letter's name, chosen by its sender: lowercase letters, digits and
/// hyphens, at most 64 characters. A letter sent again after a crash, not
/// knowing whether it went, carries the same id.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LetterId(String);

impl LetterId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LetterId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = !text.is_empty()
            && text.len() <= 64
            && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a letter id")) }
    }
}

impl From<LetterId> for String {
    fn from(id: LetterId) -> String {
        id.0
    }
}
