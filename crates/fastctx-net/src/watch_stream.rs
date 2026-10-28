//! Signed session events retain their identity across data-message boundaries.

use serde::{Deserialize, Serialize};

use crate::data::{ReadHalf, Stream};
use crate::sign::{Bytes, Signature, Signed};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Carried {
    watch_bytes: u64,
    sig: Signature,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Signed(Signed),
    Carried(Carried),
}

/// Sends the existing JSON envelope when it fits in one message of a data
/// connection as large as the line takes now; otherwise announces the
/// signed payload's length and signature, then sends its exact bytes in
/// chunks.
pub async fn send(stream: &mut Stream, signed: &Signed) -> Result<(), String> {
    let empty = Signed { data: Bytes(Vec::new()), sig: signed.sig };
    let overhead = serde_json::to_vec(&empty).expect("signed envelopes serialize").len();
    let length = signed.data.0.len();
    // Bytes are unpadded base64; the empty envelope accounts for all other JSON bytes.
    let encoded = overhead + length / 3 * 4 + if length.is_multiple_of(3) { 0 } else { length % 3 + 1 };
    if encoded <= stream.message_size() {
        return stream.say(signed).await;
    }
    stream.say(&Carried { watch_bytes: length as u64, sig: signed.sig }).await?;
    stream.send(&signed.data.0).await
}

/// Sends a signed piece as its exact bytes, announced by length and
/// signature, whatever its size: for packed pieces, which base64 would grow
/// by a third.
pub async fn send_bytes(stream: &mut Stream, signed: &Signed) -> Result<(), String> {
    stream.say(&Carried { watch_bytes: signed.data.0.len() as u64, sig: signed.sig }).await?;
    stream.send(&signed.data.0).await
}

/// Reassembles a complete signed event before the caller verifies its signature
/// and subscription sequence. A partial transfer fails without exposing a piece.
/// None once the watched machine has closed the connection, which it does
/// only on its way to answering the watch: that answer says why it ended.
pub async fn receive(stream: &mut Stream) -> Result<Option<Signed>, String> {
    match stream.hear_or_end::<Incoming>().await? {
        None => Ok(None),
        Some(Incoming::Signed(signed)) => Ok(Some(signed)),
        Some(Incoming::Carried(carried)) => {
            let mut bytes = Vec::new();
            stream
                .take(carried.watch_bytes, |part| {
                    bytes.extend_from_slice(part);
                    Ok(())
                })
                .await?;
            Ok(Some(Signed { data: Bytes(bytes), sig: carried.sig }))
        }
    }
}

/// A legacy packet keeps its byte reservation through signature verification
/// and decoding, including when the peer sent a length-prefixed large history.
pub(crate) struct Received {
    signed: Signed,
    _memory: crate::reassembly::Reservation,
}
impl std::ops::Deref for Received {
    type Target = Signed;
    fn deref(&self) -> &Signed { &self.signed }
}

pub(crate) async fn receive_half(stream: &mut ReadHalf, first: Option<serde_json::Value>) -> Result<Option<Received>, String> {
    let incoming = match first {
        Some(first) => serde_json::from_value(first).map_err(|e| format!("the legacy watch header: {e}"))?,
        None => match stream.hear_or_end::<Incoming>().await? { Some(incoming) => incoming, None => return Ok(None) },
    };
    Ok(Some(match incoming {
        Incoming::Signed(signed) => Received {
            _memory: crate::reassembly::Reservation::new(signed.data.0.len().saturating_mul(2))?, signed,
        },
        Incoming::Carried(carried) => {
            let limit = usize::try_from(carried.watch_bytes).ok().filter(|size| *size <= crate::watch_mux::MAX_PACKET)
                .ok_or("the watch packet exceeds 64 MiB")?;
            let mut bytes = crate::reassembly::Buffer::new(limit, limit)?;
            stream.take(carried.watch_bytes, |part| bytes.extend(part)).await?;
            let signed = Signed { data: Bytes(bytes.to_vec()), sig: carried.sig };
            Received { signed, _memory: bytes.into_reservation() }
        }
    }))
}
