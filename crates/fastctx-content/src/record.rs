//! One record of a stream: an item's whole state from then on, or its end.

/// The longest body of one record. Larger content is an object the record
/// refers to.
pub const RECORD_BYTES: usize = 64 * 1024;
/// The longest item identity or order key.
pub const KEY_BYTES: usize = 64;
/// The most objects one record refers to.
pub const RECORD_OBJECTS: usize = 256;
/// The most records' bytes one frame holds, before compression.
pub const FRAME_BYTES: usize = 1024 * 1024;
/// The most records one frame holds.
pub const FRAME_RECORDS: usize = 4096;

const IS: u8 = 1;
const GONE: u8 = 0;
const OBJECT_BYTES: usize = 40;

/// An object by its content: its SHA-256 and its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectRef {
    pub hash: [u8; 32],
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// The item is this from now on: its body as its kind encodes it, and
    /// the objects that body refers to.
    Is { body: Vec<u8>, objects: Vec<ObjectRef> },
    /// The item is no more.
    Gone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// What the item is known by within its stream for as long as it lasts.
    pub item: Vec<u8>,
    /// Where the item stands among the stream's items, compared as bytes. A
    /// record with another order than the item had moves it.
    pub order: Vec<u8>,
    pub state: State,
}

impl Record {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.item.is_empty() || self.item.len() > KEY_BYTES {
            return Err(format!("an item's identity is 1 to {KEY_BYTES} bytes"));
        }
        if self.order.len() > KEY_BYTES {
            return Err(format!("an item's order key is at most {KEY_BYTES} bytes"));
        }
        if let State::Is { body, objects } = &self.state {
            if body.len() > RECORD_BYTES {
                return Err(format!("a record's body is at most {RECORD_BYTES} bytes; larger content is an object"));
            }
            if objects.len() > RECORD_OBJECTS {
                return Err(format!("a record refers to at most {RECORD_OBJECTS} objects"));
            }
        }
        Ok(())
    }

    /// The bytes it takes in a frame.
    pub(crate) fn encoded_len(&self) -> usize {
        3 + self.item.len()
            + self.order.len()
            + match &self.state {
                State::Is { body, objects } => 6 + body.len() + objects.len() * OBJECT_BYTES,
                State::Gone => 0,
            }
    }

    /// Appends its bytes to a frame. The record has passed `check`.
    pub(crate) fn encode(&self, frame: &mut Vec<u8>) {
        frame.push(if matches!(self.state, State::Is { .. }) { IS } else { GONE });
        frame.push(self.item.len() as u8);
        frame.extend_from_slice(&self.item);
        frame.push(self.order.len() as u8);
        frame.extend_from_slice(&self.order);
        if let State::Is { body, objects } = &self.state {
            frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
            frame.extend_from_slice(body);
            frame.extend_from_slice(&(objects.len() as u16).to_le_bytes());
            frame.extend_from_slice(&pack(objects));
        }
    }

    /// Takes one record off the front of a frame's bytes.
    pub(crate) fn decode(frame: &mut &[u8]) -> Result<Self, String> {
        let kind = take(frame, 1)?[0];
        let length = usize::from(take(frame, 1)?[0]);
        let item = take(frame, length)?.to_vec();
        let length = usize::from(take(frame, 1)?[0]);
        let order = take(frame, length)?.to_vec();
        let state = match kind {
            GONE => State::Gone,
            IS => {
                let length = u32::from_le_bytes(take(frame, 4)?.try_into().expect("four bytes")) as usize;
                let body = take(frame, length)?.to_vec();
                let count = usize::from(u16::from_le_bytes(take(frame, 2)?.try_into().expect("two bytes")));
                State::Is { body, objects: unpack(take(frame, count * OBJECT_BYTES)?)? }
            }
            other => return Err(format!("a frame holds a record of an unknown kind {other}")),
        };
        let record = Self { item, order, state };
        record.check()?;
        Ok(record)
    }
}

fn take<'a>(frame: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
    if frame.len() < length {
        return Err("a frame ends inside a record".into());
    }
    let (taken, rest) = frame.split_at(length);
    *frame = rest;
    Ok(taken)
}

/// Object references as the index keeps them beside an item.
pub(crate) fn pack(objects: &[ObjectRef]) -> Vec<u8> {
    let mut packed = Vec::with_capacity(objects.len() * OBJECT_BYTES);
    for object in objects {
        packed.extend_from_slice(&object.hash);
        packed.extend_from_slice(&object.size.to_le_bytes());
    }
    packed
}

pub(crate) fn unpack(packed: &[u8]) -> Result<Vec<ObjectRef>, String> {
    if !packed.len().is_multiple_of(OBJECT_BYTES) {
        return Err("object references are not whole".into());
    }
    Ok(packed
        .chunks_exact(OBJECT_BYTES)
        .map(|chunk| ObjectRef {
            hash: chunk[..32].try_into().expect("32 bytes"),
            size: u64::from_le_bytes(chunk[32..].try_into().expect("eight bytes")),
        })
        .collect())
}
