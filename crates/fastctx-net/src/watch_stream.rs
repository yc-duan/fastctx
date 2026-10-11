//! Signed session events retain their identity across data-message boundaries.

use serde::{Deserialize, Serialize};

use crate::data::Stream;
use crate::sign::{Bytes, Signature, Signed};
use crate::wire::MAX_DATA_MESSAGE;

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

/// Sends the existing JSON envelope when it fits; otherwise announces the
/// signed payload's length and signature, then sends its exact bytes in chunks.
pub async fn send(stream: &mut Stream, signed: &Signed) -> Result<(), String> {
    let empty = Signed { data: Bytes(Vec::new()), sig: signed.sig };
    let overhead = serde_json::to_vec(&empty).expect("signed envelopes serialize").len();
    let length = signed.data.0.len();
    // Bytes are unpadded base64; the empty envelope accounts for all other JSON bytes.
    let encoded = overhead + length / 3 * 4 + if length.is_multiple_of(3) { 0 } else { length % 3 + 1 };
    if encoded <= MAX_DATA_MESSAGE {
        return stream.say(signed).await;
    }
    stream.say(&Carried { watch_bytes: length as u64, sig: signed.sig }).await?;
    stream.send(&signed.data.0).await
}

/// Reassembles a complete signed event before the caller verifies its signature
/// and subscription sequence. A partial transfer fails without exposing a piece.
pub async fn receive(stream: &mut Stream) -> Result<Signed, String> {
    match stream.hear::<Incoming>().await? {
        Incoming::Signed(signed) => Ok(signed),
        Incoming::Carried(carried) => {
            let mut bytes = Vec::new();
            stream
                .take(carried.watch_bytes, |part| {
                    bytes.extend_from_slice(part);
                    Ok(())
                })
                .await?;
            Ok(Signed { data: Bytes(bytes), sig: carried.sig })
        }
    }
}
