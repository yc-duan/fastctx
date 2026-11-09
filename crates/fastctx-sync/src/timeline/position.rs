//! Order-preserving keys. Each ancestry element starts with 1, its terminator
//! is 0, and all counters use big endian. An ancestor sorts before descendants.
use crate::{cursor::Sequence, identity::ObjectId, store::Error};
use fastctx_types::sync::{PAGE_BYTES, timeline::TimelinePosition};

pub fn encode(value: &TimelinePosition) -> Result<Vec<u8>, Error> {
    let size = value.display_ancestry.len().checked_mul(9).and_then(|n| n.checked_add(49)).ok_or(Error::Busy)?;
    if size > PAGE_BYTES as usize {
        return Err(Error::Invalid("timeline position exceeds one metadata page".into()));
    }
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(&value.turn.0.to_be_bytes());
    for sequence in &value.display_ancestry {
        bytes.push(1);
        bytes.extend_from_slice(&sequence.0.to_be_bytes());
    }
    bytes.push(0);
    bytes.extend_from_slice(&value.accepted.0.to_be_bytes());
    bytes.extend_from_slice(&id_bytes(&value.item));
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> Result<TimelinePosition, Error> {
    if bytes.len() < 49 || bytes.len() > PAGE_BYTES as usize {
        return Err(invalid());
    }
    let mut at = 0;
    let turn = number(bytes, &mut at)?;
    let mut ancestry = Vec::new();
    loop {
        let marker = *bytes.get(at).ok_or_else(invalid)?;
        at += 1;
        match marker {
            0 => break,
            1 => ancestry.push(Sequence(number(bytes, &mut at)?)),
            _ => return Err(invalid()),
        }
    }
    let accepted = number(bytes, &mut at)?;
    let id: [u8; 32] = bytes.get(at..).and_then(|tail| tail.try_into().ok()).ok_or_else(invalid)?;
    Ok(TimelinePosition {
        turn: Sequence(turn),
        display_ancestry: ancestry,
        accepted: Sequence(accepted),
        item: ObjectId::from_bytes(id),
    })
}
fn number(bytes: &[u8], at: &mut usize) -> Result<u64, Error> {
    let end = at.checked_add(8).ok_or_else(invalid)?;
    let value = bytes.get(*at..end).and_then(|b| b.try_into().ok()).map(u64::from_be_bytes).ok_or_else(invalid)?;
    *at = end;
    Ok(value)
}
pub(super) fn key(value: &TimelinePosition) -> Result<String, Error> {
    Ok(hex(&encode(value)?))
}
pub(super) fn id_key(id: &ObjectId) -> String {
    id.as_str()[2..].to_owned()
}
pub(super) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 15)] as char);
    }
    text
}
fn id_bytes(id: &ObjectId) -> [u8; 32] {
    let text = &id.as_str()[2..];
    std::array::from_fn(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).expect("validated object id"))
}
fn invalid() -> Error {
    Error::Invalid("invalid timeline position key".into())
}
