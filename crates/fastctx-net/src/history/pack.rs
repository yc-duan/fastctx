//! A watch's pieces for a viewer that keeps the history go as JSON in one
//! zstd frame (`world-gui.md`, 对话: the whole payload compressed). The frame
//! carries its own checksum, so a viewer that keeps what it was sent can tell
//! when the copy on its disk was damaged.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The zstd level: 3 is the format's default, measured on real histories
/// at 20 ms for a 15 MB Codex history and 0.6 s for a 297 MB Claude one
/// before slimming.
const LEVEL: i32 = 3;

/// The magic number every zstd frame begins with; JSON never does.
const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// `value` as JSON in one zstd frame.
pub fn pack(value: &impl Serialize) -> Vec<u8> {
    let json = serde_json::to_vec(value).expect("the World's statements always serialize");
    let mut encoder =
        zstd::stream::Encoder::new(Vec::with_capacity(json.len() / 4), LEVEL).expect("a zstd encoder opens in memory");
    encoder.include_checksum(true).expect("a zstd encoder takes a checksum");
    std::io::Write::write_all(&mut encoder, &json).expect("writing into memory");
    encoder.finish().expect("finishing into memory")
}

/// The value `bytes` holds: packed by `pack`, or plain JSON as a machine
/// sends that packs nothing.
pub fn unpack<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if !bytes.starts_with(&MAGIC) {
        return serde_json::from_slice(bytes).map_err(|error| error.to_string());
    }
    let json = zstd::stream::decode_all(bytes).map_err(|error| format!("unpacking: {error}"))?;
    serde_json::from_slice(&json).map_err(|error| error.to_string())
}
