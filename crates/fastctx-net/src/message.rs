//! What members say to each other, inside the sealed body of an envelope
//! (`crate::envelope`).

use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use serde::{Deserialize, Serialize};

use crate::stream_crypto::StreamKey;
use crate::wire::Carried;

/// What a call or an answer holds, sealed with the caller's session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Body {
    /// Asks the receiver to do something now.
    Call { call: RemoteCall },
    /// A call too large for a message: it follows over a data connection,
    /// as the JSON of a `RemoteCall`.
    LargeCall { carried: Carried },
    /// A call that comes with a data connection, `ticket`, whose source is
    /// the caller and whose messages are sealed with `key`: the two ends
    /// exchange what the call is about, such as the files of a copy, while
    /// the call waits for its answer.
    StreamCall { call: RemoteCall, ticket: crate::sign::Signed, key: StreamKey },
    /// The answer to the call the envelope names.
    Answer { result: RemoteResult },
    /// The answer to the call the envelope names, too large for a message:
    /// it follows over a data connection, as the JSON of a `RemoteResult`.
    LargeAnswer { carried: Carried },
}

/// What a letter holds, sealed to its receiver. The sender names it with
/// `id`: the receiver takes a letter from one sender with one id once, and
/// refuses the same id with different contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetterBody {
    pub id: LetterId,
    pub letter: Letter,
}

/// A letter's name, chosen by its sender: lowercase letters, digits and
/// hyphens, at most 64 characters. A letter sent again after a crash, not
/// knowing whether it went, carries the same id.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LetterId(String);

impl LetterId {
    /// An id as long as any can be, for measuring.
    pub fn longest() -> Self {
        Self("z".repeat(64))
    }

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
