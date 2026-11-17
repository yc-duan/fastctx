//! Asset lines: connections a device keeps open to its hub for the bulk of
//! the asset sync (`crate::assets`), beside the control connection. A line
//! carries one exchange at a time: the device asks, the hub answers, and
//! where the exchange says so bytes follow, one way or the other. Frames
//! travel with their bodies as bytes after them, objects whole; nothing on
//! a line waits for a ticket or for a connection of its own, so the round
//! trips a transfer costs do not grow with how many there are. Spoken from
//! asset sync version `crate::assets::LINES` on.
//!
//! What is said is JSON, one value a message; a value too long for one
//! message goes as bytes under a `LongHead`. Bytes go under a `BytesHead`,
//! in as many messages as the line takes, compressed where that makes them
//! shorter.

use crate::assets::{Frame, ObjectRef, Reply, Request};
use serde::{Deserialize, Serialize};

/// The most frames one exchange of a line carries, either way.
pub const FRAMES: usize = 256;

/// The most bytes of frame bodies one exchange of a line carries.
pub const FRAME_BYTES: usize = 8 << 20;

/// The most objects one `Ask::Objects` names.
pub const OBJECTS: usize = 64;

/// The longest object a line carries: a longer one travels under a ticket,
/// in chunks a transfer broken off takes up again.
pub const OBJECT_BYTES: u64 = 8 << 20;

/// The bytes of each run an object travels in on a line but its last.
pub const RUN_BYTES: u64 = 1 << 20;

/// What a device asks on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Ask {
    /// Any request the control connection carries, answered as there.
    Request { request: Request },
    /// Frames of `stream` from the one holding `from`, at most `frames` of
    /// them: answered `Told::Frames`, each frame naming its body's length
    /// in `apart`, the bodies following in order as one run of bytes.
    Frames { scope: String, stream: String, from: u64, frames: u32 },
    /// Objects of `scope`: each answered `Told::Object`, in order, its bytes
    /// following when it is given.
    Objects { scope: String, objects: Vec<ObjectRef> },
    /// An object put, or proved by its bytes: answered `Told::Send` when the
    /// hub wants its bytes, which follow and are answered `Told::Held`.
    Put {
        scope: String,
        object: ObjectRef,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        prove: bool,
    },
    /// Frames of one stream to take, in order, each naming its body's
    /// length in `apart`; the bodies follow in order as one run of bytes.
    /// Answered as `Request::TakeMany` is.
    Take { scope: String, frames: Vec<Frame> },
}

/// What the hub answers on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "told", rename_all = "snake_case")]
pub enum Told {
    /// The answer to `Ask::Request`, and to any ask the hub answers as it
    /// would on the control connection: a scope revoked, the assets sealed.
    Reply { reply: Reply },
    /// The ask was refused.
    Failed { why: String },
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
    /// The hub wants the object's bytes.
    Send,
    /// The hub holds the object, or took it as proved.
    Held,
}

/// What precedes a run of bytes: how many they are, and how many travel
/// when they travel compressed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BytesHead {
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub packed: u64,
}

/// What precedes a value too long for one message: the length of its JSON,
/// and how many bytes travel when it travels compressed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongHead {
    pub long: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub packed: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}
