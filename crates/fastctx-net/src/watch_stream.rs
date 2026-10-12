//! Signed session events retain their identity across data-message boundaries.

use serde::{Deserialize, Serialize};

use crate::data::{CHUNK, Stream};
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
/// connection; otherwise announces the signed payload's length and
/// signature, then sends its exact bytes in chunks.
pub async fn send(stream: &mut Stream, signed: &Signed) -> Result<(), String> {
    let empty = Signed { data: Bytes(Vec::new()), sig: signed.sig };
    let overhead = serde_json::to_vec(&empty).expect("signed envelopes serialize").len();
    let length = signed.data.0.len();
    // Bytes are unpadded base64; the empty envelope accounts for all other JSON bytes.
    let encoded = overhead + length / 3 * 4 + if length.is_multiple_of(3) { 0 } else { length % 3 + 1 };
    if encoded <= CHUNK {
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
