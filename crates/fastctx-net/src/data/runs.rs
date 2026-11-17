//! Whole values and runs of bytes of any length on a data connection that
//! stays open (`fastctx_types::asset_lines`). A value is JSON in one
//! message, or bytes under a `LongHead` when its JSON is too long for one;
//! a run of bytes goes under a `BytesHead`. Either travels compressed where
//! that makes it shorter.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use fastctx_types::asset_lines::{BytesHead, LongHead};

use super::{DUE_WITHIN, Message, Stream};

/// The longest JSON one message carries: a longer value goes as bytes.
pub const SHORT: usize = 128 * 1024;

/// The longest value put together from what a line carries.
pub const LONGEST: u64 = super::MAX_BODY as u64;

/// zstd's default level: what a line carries is compressed once, as it
/// goes, and read once.
const LEVEL: i32 = 3;

/// Bytes fewer than this are sent as they are.
const WORTH: usize = 512;

/// `bytes` compressed, when that makes them an eighth shorter or more.
pub fn pack(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < WORTH {
        return None;
    }
    let packed = zstd::bulk::compress(bytes, LEVEL).ok()?;
    (packed.len() + bytes.len() / 8 <= bytes.len()).then_some(packed)
}

/// The `size` bytes `packed` holds.
pub fn unpack(packed: &[u8], size: u64) -> Result<Vec<u8>, String> {
    let bytes = zstd::bulk::decompress(packed, size as usize)
        .map_err(|error| format!("unpacking what a line carried: {error}"))?;
    if bytes.len() as u64 != size {
        return Err("what a line carried unpacks to another length than its head said".into());
    }
    Ok(bytes)
}

/// The head `text` is, when it is one of a value sent as bytes: no value
/// said on a line begins as a head does.
pub fn long_head(text: &str) -> Option<LongHead> {
    text.starts_with("{\"long\":").then(|| serde_json::from_str(text).ok()).flatten()
}

/// The head of `json` as it travels, and the bytes that travel.
pub fn long(json: &[u8]) -> (LongHead, Option<Vec<u8>>) {
    let packed = pack(json);
    (LongHead { long: json.len() as u64, packed: packed.as_ref().map_or(0, |packed| packed.len() as u64) }, packed)
}

/// The head of a run of `bytes` as it travels, and the bytes that travel;
/// `plain` those known not to compress.
pub fn run(bytes: &[u8], plain: bool) -> (BytesHead, Option<Vec<u8>>) {
    let packed = if plain { None } else { pack(bytes) };
    (BytesHead { bytes: bytes.len() as u64, packed: packed.as_ref().map_or(0, |packed| packed.len() as u64) }, packed)
}

impl Stream {
    /// Says `value`, however long its JSON.
    pub async fn tell(&mut self, value: &impl Serialize) -> Result<(), String> {
        let json = serde_json::to_vec(value).expect("what is said on a line serializes");
        if json.len() <= SHORT {
            let text = String::from_utf8(json).expect("JSON is UTF-8");
            return self.put(WsMessage::Text(text.into())).await;
        }
        let (head, packed) = long(&json);
        self.say(&head).await?;
        self.send(packed.as_deref().unwrap_or(&json)).await
    }

    /// The next value the other end says, a step due as `hear_step`'s is.
    pub async fn told<T: DeserializeOwned>(&mut self, step: &str) -> Result<T, String> {
        let text = self.word_step(step).await?;
        let Some(head) = long_head(&text) else {
            return serde_json::from_str(&text).map_err(|error| format!("what the other end said: {error}"));
        };
        if head.long > LONGEST || head.packed > LONGEST {
            return Err(format!("the other end said {} bytes at once; at most {LONGEST} are put together", head.long));
        }
        let travelling = if head.packed > 0 { head.packed } else { head.long };
        let mut bytes = Vec::with_capacity(travelling as usize);
        self.take(travelling, |piece| {
            bytes.extend_from_slice(piece);
            Ok(())
        })
        .await?;
        let json = if head.packed > 0 { unpack(&bytes, head.long)? } else { bytes };
        serde_json::from_slice(&json).map_err(|error| format!("what the other end said: {error}"))
    }

    /// Sends a run of bytes under its head; `plain` those known not to
    /// compress.
    pub async fn give(&mut self, bytes: &[u8], plain: bool) -> Result<(), String> {
        let (head, packed) = run(bytes, plain);
        self.say(&head).await?;
        self.send(packed.as_deref().unwrap_or(bytes)).await
    }

    /// Takes a run of at most `most` bytes, a step due as `hear_step`'s is.
    pub async fn given(&mut self, most: u64, step: &str) -> Result<Vec<u8>, String> {
        let text = self.word_step(step).await?;
        let head: BytesHead = serde_json::from_str(&text).map_err(|error| format!("the head of {step}: {error}"))?;
        if head.bytes > most || head.packed > most {
            return Err(format!("{step} came as {} bytes; at most {most} were due", head.bytes));
        }
        let travelling = if head.packed > 0 { head.packed } else { head.bytes };
        let mut bytes = Vec::with_capacity(travelling as usize);
        self.take(travelling, |piece| {
            bytes.extend_from_slice(piece);
            Ok(())
        })
        .await?;
        if head.packed > 0 { unpack(&bytes, head.bytes) } else { Ok(bytes) }
    }

    /// The next text the other end says, within `DUE_WITHIN` of its last
    /// report of work toward it: the line's heartbeats alone do not keep it
    /// waited for.
    async fn word_step(&mut self, step: &str) -> Result<String, String> {
        let mut progress = self.line.receipt.subscribe();
        let began = tokio::time::Instant::now();
        loop {
            let worked = (*progress.borrow_and_update()).unwrap_or(began).max(began);
            tokio::select! {
                heard = self.word() => return heard,
                changed = progress.changed() => {
                    changed.map_err(|_| "the other end's work notifications ended".to_string())?;
                },
                () = tokio::time::sleep_until(worked + DUE_WITHIN) => {
                    return Err(format!("{step} did not come, and the other end reported no work toward it, for {} s", DUE_WITHIN.as_secs()));
                },
            }
        }
    }

    async fn word(&mut self) -> Result<String, String> {
        if !self.pending.is_empty() {
            return Err("bytes came where the other end was to say something".to_string());
        }
        match self.next().await? {
            Some(Message::Word(text)) => Ok(text),
            Some(Message::Bytes(_)) => Err("bytes came where the other end was to say something".to_string()),
            None => Err("the other end left".to_string()),
        }
    }

    /// Whether the connection is over already: one kept open and unused is
    /// looked at before it is used again.
    pub fn ended_already(&self) -> bool {
        self.line.over()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_compresses_is_packed_and_unpacks_whole() {
        let text = "a line of a tool's output\n".repeat(4096).into_bytes();
        let (head, packed) = run(&text, false);
        let packed = packed.expect("text compresses");
        assert_eq!(head.bytes, text.len() as u64);
        assert_eq!(head.packed, packed.len() as u64);
        assert!(packed.len() < text.len() / 8);
        assert_eq!(unpack(&packed, head.bytes).expect("unpacks"), text);
    }

    #[test]
    fn what_does_not_compress_travels_as_it_is() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let noise: Vec<u8> = (0..65_536)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect();
        let (head, packed) = run(&noise, false);
        assert!(packed.is_none());
        assert_eq!(head.packed, 0);
        let (head, packed) = run(b"short", false);
        assert!(packed.is_none());
        assert_eq!(head.bytes, 5);
    }

    #[test]
    fn a_head_is_told_from_a_value() {
        assert_eq!(long_head("{\"long\":10,\"packed\":4}"), Some(LongHead { long: 10, packed: 4 }));
        assert_eq!(long_head("{\"told\":\"held\"}"), None);
    }
}
