//! What the old hub and the new place say to each other once the old hub
//! dialed it (`fastctx_net::wire::Intent::Handover`): one JSON value in a
//! binary WebSocket message each, and a file's bytes in the binary
//! messages after its `Put`. Either side that is busy, listing or checking
//! what it holds, says `Working` every heartbeat, so the other tells a busy
//! end from a gone one by the protocol's own silence.

use std::future::Future;

use fastctx_net::sign::Digest;
use fastctx_net::wire::{HEARTBEAT, SILENCE};
use futures_util::{Sink, SinkExt as _, Stream, StreamExt as _};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};

use super::files::{Entry, Part};

/// The most bytes of a file in one message.
pub(crate) const CHUNK: usize = fastctx_net::wire::MAX_DATA_MESSAGE;

/// The largest listing in one `Files` message, well within a message.
const LISTING: usize = fastctx_net::wire::MAX_DATA_MESSAGE / 2;

/// How long a piece of a file is where one is sent as what changed of the
/// one the other end holds (`ToNew::Patch`): a database's page is no longer
/// than this, and one page changed sends one piece.
pub(crate) const PIECE: u32 = 64 << 10;

/// How many pieces' digests one message carries.
pub(crate) const DIGESTS: usize = 8192;

/// From the old hub to the new place.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "f")]
pub(crate) enum ToNew {
    /// Say what you hold. The sender must carry all settings in its database.
    Holds {
        #[serde(default)]
        sqlite: bool,
        /// This sender hands over the hub's core alone and sends the owner
        /// libraries once it serves no more (`files::Part`). An older sender
        /// omits it and would stop the hub for all of them: it is refused.
        #[serde(default)]
        apart: bool,
        /// Of which part; none, of everything.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<Part>,
    },
    /// A file, whose `bytes` follow, then its `Sum`. `packed`, each message
    /// of them begins with how it is carried (`pack`).
    Put {
        path: String,
        bytes: u64,
        #[serde(default)]
        packed: bool,
    },
    /// The SHA-256 of the file just sent: the new place keeps it only when
    /// what came has this digest.
    Sum {
        sha256: Digest,
    },
    /// Say what the file you hold at `path` is, `piece` bytes at a time.
    Pieces {
        path: String,
        piece: u32,
    },
    /// The file at `path`, `bytes` long, made of the one you hold there: the
    /// pieces of `changed` — runs of them, each by its first and how many —
    /// are the bytes that follow, in order; the others are as yours are.
    /// Then its `Sum`, of the whole: kept only when what was made has it.
    Patch {
        path: String,
        bytes: u64,
        piece: u32,
        changed: Vec<(u64, u64)>,
    },
    /// A directory, made where it is missing.
    Dir {
        path: String,
    },
    /// A file or a directory with all it holds.
    Remove {
        path: String,
    },
    /// Say the digest of what you hold: of `part`, or of everything.
    Check {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part: Option<Part>,
    },
    /// The old hub stopped and sent its core: take over, when the core you
    /// hold has this digest. `libraries` are the owner libraries it keeps
    /// and sends once it serves no more: each is opened when its databases
    /// came, and its large objects follow.
    Commit {
        digest: Digest,
        self_registration: bool,
        record: Box<Record>,
        #[serde(default)]
        libraries: Vec<String>,
    },
    /// The owner library `library` was sent whole: open it, when what you
    /// hold of it has this digest.
    Arrived {
        library: String,
        digest: Digest,
    },
    /// Say which owner libraries are still to come, and which have large
    /// objects still to follow them.
    Coming,
    /// Say which of these large objects of the owner library `library`,
    /// open with you, you do not hold.
    Follow {
        library: String,
        objects: Vec<Object>,
    },
    /// A large object of the library last asked after, whose `bytes` follow
    /// as a `Put`'s do: kept when what came is the object `sha256` names.
    Object {
        sha256: Digest,
        bytes: u64,
    },
    /// Every large object of `library` something refers to is with you.
    Followed {
        library: String,
    },
    /// The old hub recorded the switch: serve from now on.
    Confirm,
    /// The move is called off: give it up.
    Cancel,
    /// The old place after the switch: how it stands.
    Old {
        forwarding: bool,
        deleted: bool,
    },
    /// The old place deleted its data, as asked.
    Deleted,
    Working,
}

/// From the new place to the old hub.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "f")]
pub(crate) enum ToOld {
    /// What it holds: whether it took over already, then the listing in
    /// `Files` messages, then `End`.
    Holds {
        committed: bool,
        /// This receiving process can open the hub's SQLite records. Older
        /// senders ignore it; older receivers omit it and cannot take them.
        #[serde(default)]
        sqlite: bool,
        /// This receiving process takes the hub's core over and opens each
        /// owner library when it came. An older receiver omits it and would
        /// take a hub without them for one put back from an older copy.
        #[serde(default)]
        apart: bool,
    },
    Files {
        entries: Vec<Entry>,
    },
    End,
    Checked {
        digest: Digest,
    },
    /// How long the file asked after is, none when no file is there; the
    /// SHA-256 of each of its pieces follows, one after another, in messages
    /// of `DIGESTS` of them.
    Pieces {
        bytes: Option<u64>,
    },
    Committed,
    Confirmed,
    Cancelled,
    /// The owner library came whole and is open.
    Opened,
    /// The owner libraries still to come, and those that came, or are to,
    /// whose large objects are still to follow.
    Awaited {
        libraries: Vec<String>,
        #[serde(default)]
        objects: Vec<String>,
    },
    /// Of the objects asked after, those not held, by where each was in the
    /// asking.
    Lacking {
        objects: Vec<u32>,
    },
    /// The library's large objects are noted as come.
    Kept,
    /// How many machines and devices still come through the old place.
    Following {
        machines: usize,
        devices: usize,
    },
    /// Stop forwarding; `early` when not every machine had followed.
    Stop {
        early: bool,
    },
    /// Delete the data you kept.
    Delete,
    /// It will not do what was asked, and why.
    Refused {
        reason: String,
    },
    Working,
}

/// A large object, by its hash and its length.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Object {
    pub sha256: Digest,
    pub bytes: u64,
}

/// What the old hub tells the new place of the move when it hands over.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub by: fastctx_types::AccountName,
    pub started: jiff::Timestamp,
}

pub(crate) trait Frame: Serialize + DeserializeOwned {
    fn working() -> Self;
    fn is_working(&self) -> bool;
}

impl Frame for ToNew {
    fn working() -> Self {
        ToNew::Working
    }

    fn is_working(&self) -> bool {
        matches!(self, ToNew::Working)
    }
}

impl Frame for ToOld {
    fn working() -> Self {
        ToOld::Working
    }

    fn is_working(&self) -> bool {
        matches!(self, ToOld::Working)
    }
}

/// What came: a frame, or a piece of a file.
pub(crate) enum Got<T> {
    Frame(T),
    Bytes(Vec<u8>),
}

pub(crate) async fn send<S, T: Serialize>(socket: &mut S, frame: &T) -> Result<(), String>
where
    S: Sink<WsMessage, Error = WsError> + Unpin,
{
    send_bytes(socket, fastctx_net::wire::encode(frame)).await
}

/// Writes one message, taking no longer than the silence: the other hub
/// that takes nothing is as gone as one that says nothing, and a write that
/// waits on it would keep its side from hearing that.
pub(crate) async fn send_bytes<S>(socket: &mut S, bytes: Vec<u8>) -> Result<(), String>
where
    S: Sink<WsMessage, Error = WsError> + Unpin,
{
    match tokio::time::timeout(SILENCE, socket.send(WsMessage::Binary(bytes.into()))).await {
        Ok(sent) => sent.map_err(|error| error.to_string()),
        Err(_) => Err(format!("the other hub took nothing for {} s", SILENCE.as_secs())),
    }
}

/// The next frame or piece of a file, past any `Working`; an error when the
/// other side closed, or said nothing for `SILENCE`. A message that is not
/// a frame is a piece of a file only when `bytes` says one is expected.
pub(crate) async fn recv<S, T: Frame>(socket: &mut S, bytes: bool) -> Result<Got<T>, String>
where
    S: Stream<Item = Result<WsMessage, WsError>> + Unpin,
{
    loop {
        let message = match tokio::time::timeout(SILENCE, socket.next()).await {
            Err(_) => return Err(format!("nothing was heard from the other hub for {} s", SILENCE.as_secs())),
            Ok(None | Some(Ok(WsMessage::Close(_)))) => return Err("the other hub closed the connection".to_string()),
            Ok(Some(Err(error))) => return Err(fastctx_net::dial::describe(&error)),
            Ok(Some(Ok(message))) => message,
        };
        let WsMessage::Binary(data) = message else { continue };
        if bytes {
            return Ok(Got::Bytes(data.to_vec()));
        }
        let frame: T = decode(&data)?;
        if !frame.is_working() {
            return Ok(Got::Frame(frame));
        }
    }
}

/// A frame of the handover, or the reason the other hub refused, which it
/// says as any hub refuses a connection.
fn decode<T: Frame>(data: &[u8]) -> Result<T, String> {
    serde_json::from_slice(data).map_err(|error| match serde_json::from_slice::<fastctx_net::wire::Down>(data) {
        Ok(fastctx_net::wire::Down::Refused { reason }) => format!("the other hub refused: {reason}"),
        _ => format!("an unreadable frame: {error}"),
    })
}

/// What one message read off a connection says: a frame, or nothing to
/// act on (`Working`, a ping); an error when the connection ended.
pub(crate) fn parse<T: Frame>(message: Option<Result<WsMessage, WsError>>) -> Result<Option<T>, String> {
    match message {
        None | Some(Ok(WsMessage::Close(_))) => Err("the other hub closed the connection".to_string()),
        Some(Err(error)) => Err(fastctx_net::dial::describe(&error)),
        Some(Ok(WsMessage::Binary(data))) => {
            let frame: T = decode(&data)?;
            Ok(Some(frame).filter(|frame| !frame.is_working()))
        }
        Some(Ok(_)) => Ok(None),
    }
}

/// The next frame, refusing a piece of a file.
pub(crate) async fn frame<S, T: Frame>(socket: &mut S) -> Result<T, String>
where
    S: Stream<Item = Result<WsMessage, WsError>> + Unpin,
{
    match recv(socket, false).await? {
        Got::Frame(frame) => Ok(frame),
        Got::Bytes(_) => Err("bytes came where a frame was due".to_string()),
    }
}

/// Runs `work`, saying `Working` every heartbeat until it is done.
pub(crate) async fn working<S, T: Frame, R>(socket: &mut S, work: impl Future<Output = R>) -> Result<R, String>
where
    S: Sink<WsMessage, Error = WsError> + Unpin,
{
    tokio::pin!(work);
    loop {
        tokio::select! {
            done = &mut work => return Ok(done),
            () = tokio::time::sleep(HEARTBEAT) => send(socket, &T::working()).await?,
        }
    }
}

/// Sends a listing as `Files` messages, each well within a message, then
/// `End`.
pub(crate) async fn send_listing<S>(socket: &mut S, entries: Vec<Entry>) -> Result<(), String>
where
    S: Sink<WsMessage, Error = WsError> + Unpin,
{
    let mut part = Vec::new();
    let mut size = 0;
    for entry in entries {
        let added = fastctx_net::wire::encode(&entry).len() + 1;
        if size + added > LISTING && !part.is_empty() {
            send(socket, &ToOld::Files { entries: std::mem::take(&mut part) }).await?;
            size = 0;
        }
        size += added;
        part.push(entry);
    }
    if !part.is_empty() {
        send(socket, &ToOld::Files { entries: part }).await?;
    }
    send(socket, &ToOld::End).await
}

/// One message of a file's bytes as it is carried where its `Put` says
/// `packed`: a byte that says how, then the bytes as they are, or how many
/// they are and the bytes compressed.
pub(crate) fn pack(piece: Vec<u8>) -> Vec<u8> {
    match fastctx_net::data::runs::pack(&piece) {
        Some(packed) => {
            let mut message = Vec::with_capacity(packed.len() + 5);
            message.push(1);
            message.extend_from_slice(&(piece.len() as u32).to_le_bytes());
            message.extend_from_slice(&packed);
            message
        }
        None => {
            let mut message = Vec::with_capacity(piece.len() + 1);
            message.push(0);
            message.extend_from_slice(&piece);
            message
        }
    }
}

/// The bytes a message made by `pack` carries.
pub(crate) fn unpack(message: &[u8]) -> Result<Vec<u8>, String> {
    match message.split_first() {
        Some((0, bytes)) => Ok(bytes.to_vec()),
        Some((1, rest)) if rest.len() >= 4 => {
            let size = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
            if size as usize > CHUNK {
                return Err("a piece of a file says it is longer than a piece may be".to_string());
            }
            fastctx_net::data::runs::unpack(&rest[4..], u64::from(size))
        }
        _ => Err("a piece of a file came in no form this hub reads".to_string()),
    }
}
