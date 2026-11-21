//! What a machine that orders a conversation's streams gives of them to the
//! machines that hold a copy (`crate::assets::DIRECT`): the frames it keeps
//! and the objects they refer to, over a data connection a call owns
//! (`crate::remote::RemoteCall::Assets`), directly where the two are linked
//! and through the hub where they are not.
//!
//! One exchange goes on the connection at a time, as on an asset line
//! (`crate::asset_lines`): the holder asks, the machine answers, and bytes
//! follow where the exchange says so. Every ask carries the hub's word that
//! the asking device holds the scope: the machine judges nobody's right
//! itself, and gives nothing without it.

use crate::assets::{Bytes, Frame, ObjectRef};
use serde::{Deserialize, Serialize};

/// The most scopes one `Ask::Heads` names, and one permit grants.
pub const SCOPES: usize = 256;

/// The longest object a machine takes whole from the machine that keeps a
/// conversation's frames, held in memory until it is seen to be the object;
/// a longer one is asked of the hub, or read where the conversation lives
/// as it is opened. A hub keeps its copy of a longer one (`assets::VOUCH`).
pub const OBJECT_BYTES: u64 = 64 << 20;

/// The hub's word that a device holds scopes (`crate::assets::Reply::Permits`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permit {
    /// Until when it stands, in the hub's milliseconds.
    pub until: i64,
    pub granted: Vec<String>,
    pub word: Bytes,
}

/// What a holder asks of the machine that orders a stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Ask {
    /// Where the streams this machine orders of `scopes` stand.
    Heads { permit: Permit, scopes: Vec<String> },
    /// Frames of `stream` from the one holding `from`, at most `frames` of
    /// them: answered `Told::Frames`, each frame naming its body's length in
    /// `apart`, the bodies following in order as one run of bytes.
    Frames { permit: Permit, scope: String, stream: String, from: u64, frames: u32 },
    /// Objects the frames of `scope` refer to: each answered `Told::Object`,
    /// in order, its bytes following when it is given.
    Objects { permit: Permit, scope: String, objects: Vec<ObjectRef> },
}

/// What the machine answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "told", rename_all = "snake_case")]
pub enum Told {
    Heads { heads: Vec<Head> },
    /// Frames, their bodies following in order as one run of bytes.
    Frames { frames: Vec<Frame> },
    /// The object `index` of an `Ask::Objects`: its bytes follow when it is
    /// `given`, and `why` says why not otherwise.
    Object {
        index: u32,
        given: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        why: String,
    },
    /// The ask was refused: the permit does not stand, or what was asked for
    /// is not this machine's to give.
    Failed { why: String },
    /// As much is being given as this machine gives at once: asked again
    /// shortly.
    Busy,
}

/// Where a stream stands on the machine that orders it: the sequence its
/// next frame takes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub scope: String,
    pub stream: String,
    pub next: u64,
}
