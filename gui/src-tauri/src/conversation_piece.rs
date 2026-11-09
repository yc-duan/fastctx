use crate::{epoch_ms, transcript};
use serde::Serialize;

/// One piece of a watched conversation, as the page receives it: the page
/// holds a part of it near its reader (`transcript.rs`).
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum Piece {
    Resumed,
    /// Events kept after all the page was given, from `start` on.
    Extended {
        start: usize,
        events: Vec<Box<serde_json::value::RawValue>>,
    },
    /// The page's part is now this, in place of all it holds: the history
    /// as its machine has it now differs from what was given.
    Window {
        window: transcript::Window,
    },
    /// What came before the page's part arrived: what it holds stays, its
    /// places moved along by `offset` events and `turns_added` turns.
    Rebased {
        generation: u64,
        offset: usize,
        turns_added: usize,
    },
    /// The live part broke and is being read again: why, and since when
    /// (milliseconds since the Unix epoch).
    Interrupted {
        error: Option<String>,
        since: u64,
    },
    /// One thing as it happened, the `at`th event of the conversation.
    Event {
        at: usize,
        event: Box<serde_json::value::RawValue>,
    },
    /// Whether the conversation works now, and on which turn, as its
    /// machine holds it; told after every event before it. A machine too
    /// old to tell this never sends it.
    Now {
        busy: bool,
        turn: Option<String>,
        kept: Vec<fastctx_types::session::KeptWords>,
    },
    /// The watch is over: the node ended it, or the connection broke.
    Ended {
        error: Option<String>,
    },
    /// The conversation was deleted: nothing more of it comes.
    Deleted {
        error: String,
    },
    /// Its machine refused this account: watching again changes nothing
    /// until the account may see it.
    Refused {
        error: String,
    },
}

impl Piece {
    pub(crate) fn interrupted(interruption: &fastctx_client::Interruption) -> Self {
        Piece::Interrupted { error: Some(interruption.why.clone()), since: epoch_ms(interruption.since) }
    }

    /// How a watch that failed for `error` is over.
    pub(crate) fn failed(error: &fastctx_client::Error) -> Self {
        if error.is_deleted() {
            Piece::Deleted { error: error.to_string() }
        } else if error.is_refused() {
            Piece::Refused { error: error.to_string() }
        } else {
            Piece::Ended { error: Some(error.to_string()) }
        }
    }

    /// Whether nothing comes after it.
    pub(crate) fn last(&self) -> bool {
        matches!(self, Piece::Ended { .. } | Piece::Deleted { .. } | Piece::Refused { .. })
    }
}
