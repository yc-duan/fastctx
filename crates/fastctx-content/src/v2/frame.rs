//! A frame of the second version (§2.2): a header, its signature and the
//! compressed records, each kept exactly as made and passed on unchanged.
//!
//! The header names the scope, stream and epoch, where the frame starts and
//! how many records it holds, the chain value before it and the digest of
//! its records before compression, and the device and key that ordered it.
//! The ordering device signs the header bytes; the digest makes that cover
//! the records. The chain value takes in scope, stream and epoch, so a frame
//! cannot be passed off as another stream's or another epoch's.

use crate::record::{FRAME_BYTES, FRAME_OBJECTS, FRAME_RECORDS, Record, State};
use sha2::{Digest, Sha256};

/// The header's format.
pub const FORMAT: u8 = 1;
/// Compression that costs less than the disk and the line it saves.
const LEVEL: i32 = 1;

/// A record of a kind at a schema (§2.4): what the kind's registry number
/// and the version of its encoding say of the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindRecord {
    pub kind: u16,
    pub schema: u16,
    pub record: Record,
}

impl KindRecord {
    fn encoded_len(&self) -> usize {
        4 + self.record.encoded_len()
    }

    fn encode(&self, frame: &mut Vec<u8>) {
        frame.extend_from_slice(&self.kind.to_le_bytes());
        frame.extend_from_slice(&self.schema.to_le_bytes());
        self.record.encode(frame);
    }

    fn decode(frame: &mut &[u8]) -> Result<Self, String> {
        if frame.len() < 4 {
            return Err("a frame ends inside a record".into());
        }
        let kind = u16::from_le_bytes([frame[0], frame[1]]);
        let schema = u16::from_le_bytes([frame[2], frame[3]]);
        *frame = &frame[4..];
        Ok(Self { kind, schema, record: Record::decode(frame)? })
    }
}

/// What a frame's header says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub scope: String,
    pub stream: String,
    pub epoch: u64,
    pub first: u64,
    pub count: u32,
    pub previous: [u8; 32],
    pub digest: [u8; 32],
    pub device: [u8; 32],
    pub key: [u8; 32],
}

fn put(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&(field.len() as u32).to_le_bytes());
    bytes.extend_from_slice(field);
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
    if bytes.len() < length {
        return Err("a frame header ends early".into());
    }
    let (taken, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(taken)
}

fn take_field<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8], String> {
    let length = u32::from_le_bytes(take(bytes, 4)?.try_into().expect("four bytes")) as usize;
    take(bytes, length)
}

impl Header {
    /// The bytes that are signed and kept.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![FORMAT];
        put(&mut bytes, self.scope.as_bytes());
        put(&mut bytes, self.stream.as_bytes());
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.first.to_le_bytes());
        bytes.extend_from_slice(&self.count.to_le_bytes());
        bytes.extend_from_slice(&self.previous);
        bytes.extend_from_slice(&self.digest);
        bytes.extend_from_slice(&self.device);
        bytes.extend_from_slice(&self.key);
        bytes
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self, String> {
        let bytes = &mut bytes;
        if take(bytes, 1)?[0] != FORMAT {
            return Err("a frame header of an unknown format".into());
        }
        let text = |field: &[u8]| String::from_utf8(field.to_vec()).map_err(|_| "a frame header name is not UTF-8".to_string());
        let scope = text(take_field(bytes)?)?;
        let stream = text(take_field(bytes)?)?;
        let number = |bytes: &mut &[u8], width: usize| -> Result<u64, String> {
            let field = take(bytes, width)?;
            let mut wide = [0u8; 8];
            wide[..width].copy_from_slice(field);
            Ok(u64::from_le_bytes(wide))
        };
        let epoch = number(bytes, 8)?;
        let first = number(bytes, 8)?;
        let count = number(bytes, 4)? as u32;
        let array = |bytes: &mut &[u8]| -> Result<[u8; 32], String> { Ok(take(bytes, 32)?.try_into().expect("32 bytes")) };
        let header = Self {
            scope,
            stream,
            epoch,
            first,
            count,
            previous: array(bytes)?,
            digest: array(bytes)?,
            device: array(bytes)?,
            key: array(bytes)?,
        };
        if !bytes.is_empty() {
            return Err("a frame header has bytes past its end".into());
        }
        Ok(header)
    }

    /// The stream's chain value after this frame.
    pub fn chain(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"fastctx-asset-chain/1");
        let mut fields = Vec::new();
        put(&mut fields, self.scope.as_bytes());
        put(&mut fields, self.stream.as_bytes());
        hasher.update(&fields);
        hasher.update(self.epoch.to_le_bytes());
        hasher.update(self.first.to_le_bytes());
        hasher.update(self.count.to_le_bytes());
        hasher.update(self.previous);
        hasher.update(self.digest);
        hasher.finalize().into()
    }
}

/// Records packed for a frame: how many, their digest, and their bytes
/// compressed.
pub(crate) struct Sealed {
    pub count: u32,
    pub digest: [u8; 32],
    pub body: Vec<u8>,
    pub records: std::ops::Range<usize>,
}

/// Packs records into frames within the bounds (1 MiB of records before
/// compression, 4,096 records, or 4,096 distinct objects), in order.
pub(crate) fn seal(records: &[KindRecord]) -> Result<Vec<Sealed>, String> {
    let mut sealed = Vec::new();
    let mut raw = Vec::new();
    let mut objects = std::collections::HashSet::new();
    let mut start = 0;
    for (index, record) in records.iter().enumerate() {
        let count = index - start;
        let referred = referred(&record.record);
        let more = referred.iter().filter(|hash| !objects.contains(*hash)).count();
        if count == FRAME_RECORDS
            || (count > 0 && raw.len() + record.encoded_len() > FRAME_BYTES)
            || (count > 0 && objects.len() + more > FRAME_OBJECTS)
        {
            sealed.push(close(&raw, start..index)?);
            raw.clear();
            objects.clear();
            start = index;
        }
        objects.extend(referred.iter().copied());
        record.encode(&mut raw);
    }
    if start < records.len() {
        sealed.push(close(&raw, start..records.len())?);
    }
    Ok(sealed)
}

fn referred(record: &Record) -> Vec<[u8; 32]> {
    match &record.state {
        State::Is { objects, .. } => objects.iter().map(|object| object.hash).collect(),
        State::Gone => Vec::new(),
    }
}

fn close(raw: &[u8], records: std::ops::Range<usize>) -> Result<Sealed, String> {
    let body = zstd::bulk::compress(raw, LEVEL).map_err(|error| format!("compressing a frame: {error}"))?;
    Ok(Sealed { count: records.len() as u32, digest: Sha256::digest(raw).into(), body, records })
}

/// The records of a frame's body, checked against the header's count and
/// digest.
pub fn open(header: &Header, body: &[u8]) -> Result<Vec<KindRecord>, String> {
    // The bounds a frame is made within (§2.2) are checked before anything
    // is sized by what it says.
    if header.count == 0 || header.count as usize > FRAME_RECORDS {
        return Err(format!("a frame holds 1 to {FRAME_RECORDS} records"));
    }
    let raw = zstd::bulk::decompress(body, FRAME_BYTES)
        .map_err(|error| format!("a frame does not decompress within {FRAME_BYTES} bytes: {error}"))?;
    if <[u8; 32]>::from(Sha256::digest(&raw)) != header.digest {
        return Err("a frame's records do not match its header's digest".into());
    }
    let mut rest = raw.as_slice();
    let mut records = Vec::new();
    while !rest.is_empty() {
        if records.len() == header.count as usize {
            return Err(format!("a frame said {} records and holds more", header.count));
        }
        records.push(KindRecord::decode(&mut rest)?);
    }
    if records.len() != header.count as usize {
        return Err(format!("a frame said {} records and holds {}", header.count, records.len()));
    }
    let objects: std::collections::HashSet<[u8; 32]> = records.iter().flat_map(|record| referred(&record.record)).collect();
    if objects.len() > FRAME_OBJECTS {
        return Err(format!("a frame refers to at most {FRAME_OBJECTS} distinct objects"));
    }
    Ok(records)
}

/// Signs a frame header as the stream's ordering device (`Domain::SyncSourceBatch`).
pub trait Signer: Send + Sync {
    fn device(&self) -> [u8; 32];
    fn key(&self) -> [u8; 32];
    fn sign(&self, header: &[u8]) -> Vec<u8>;
}
