//! A frame: some records' exact bytes, as they are stored and as they go to
//! other holders. Each frame is chained to the one before it in its stream,
//! so a holder that has a stream's head knows every frame before it.

use crate::record::{FRAME_BYTES, FRAME_RECORDS, Record};
use sha2::{Digest, Sha256};

/// Compression that costs less than the disk and the line it saves.
const LEVEL: i32 = 1;

pub(crate) struct Frame {
    pub count: u32,
    /// SHA-256 of the records' bytes before compression.
    pub digest: [u8; 32],
    /// The records' bytes, compressed.
    pub bytes: Vec<u8>,
}

/// Packs checked records into frames within the bounds, in order.
pub(crate) fn frames(records: &[Record]) -> Result<Vec<Frame>, String> {
    let mut frames = Vec::new();
    let mut raw = Vec::new();
    let mut count = 0u32;
    for record in records {
        if count as usize == FRAME_RECORDS || (count > 0 && raw.len() + record.encoded_len() > FRAME_BYTES) {
            frames.push(seal(&raw, count)?);
            raw.clear();
            count = 0;
        }
        record.encode(&mut raw);
        count += 1;
    }
    if count > 0 {
        frames.push(seal(&raw, count)?);
    }
    Ok(frames)
}

fn seal(raw: &[u8], count: u32) -> Result<Frame, String> {
    let bytes = zstd::bulk::compress(raw, LEVEL).map_err(|error| format!("compressing a frame: {error}"))?;
    Ok(Frame { count, digest: Sha256::digest(raw).into(), bytes })
}

/// The stream's head after a frame: what came before it, where it starts,
/// how many records it holds and what they are.
pub(crate) fn chain(previous: Option<&[u8; 32]>, first: u64, count: u32, digest: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(previous.unwrap_or(&[0; 32]));
    hasher.update(first.to_le_bytes());
    hasher.update(count.to_le_bytes());
    hasher.update(digest);
    hasher.finalize().into()
}

/// The records of a stored frame, and the digest of their bytes.
pub(crate) fn records(bytes: &[u8], count: u32) -> Result<(Vec<Record>, [u8; 32]), String> {
    let raw = zstd::bulk::decompress(bytes, FRAME_BYTES + crate::record::RECORD_BYTES)
        .map_err(|error| format!("a frame does not decompress: {error}"))?;
    let mut rest = raw.as_slice();
    let mut records = Vec::with_capacity(count as usize);
    while !rest.is_empty() {
        records.push(Record::decode(&mut rest)?);
    }
    if records.len() != count as usize {
        return Err(format!("a frame said {count} records and holds {}", records.len()));
    }
    Ok((records, Sha256::digest(&raw).into()))
}
