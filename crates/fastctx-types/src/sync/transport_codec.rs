//! Bounded v1 binary envelopes, independent of WebSocket and storage IO.
//!
//! Validate the 16-byte prefix before reserving a frame body. A complete frame
//! is exactly one binary WebSocket message; fragmentation must also be bounded
//! by the network adapter. Payloads borrow their input rather than being copied.

use super::{identity::Digest, session::*};
use serde::Serialize;
use std::io::{self, Write};

pub const MAGIC: [u8; 4] = *b"FXST";
pub const VERSION: u8 = 1;
pub const PREFIX_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameTag {
    TransportControl = 1,
    BlockData = 2,
    MetadataPart = 3,
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("unsupported sync envelope")]
    UnsupportedProtocol,
    #[error("unknown sync frame tag {0}")]
    UnknownTag(u8),
    #[error("sync frame exceeds negotiated bounds")]
    TooLarge,
    #[error("sync frame length mismatch")]
    LengthMismatch,
    #[error("invalid sync frame: {0}")]
    InvalidFrame(String),
    #[error("sync frame payload digest mismatch")]
    CorruptFrame,
    #[error("metadata reassembly budget is occupied")]
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramePrefix {
    pub tag: FrameTag,
    pub header_bytes: u32,
    pub payload_bytes: u32,
}
impl FramePrefix {
    /// Examines only the fixed prefix; no input-controlled allocation occurs.
    pub fn decode(bytes: &[u8], limits: &TransportLimits) -> Result<Self, CodecError> {
        limits.validate().map_err(CodecError::InvalidFrame)?;
        if bytes.len() != PREFIX_BYTES {
            return Err(CodecError::LengthMismatch);
        }
        if bytes[..4] != MAGIC || bytes[4] != VERSION || bytes[6..8] != [0, 0] {
            return Err(CodecError::UnsupportedProtocol);
        }
        let tag = match bytes[5] {
            1 => FrameTag::TransportControl,
            2 => FrameTag::BlockData,
            3 => FrameTag::MetadataPart,
            other => return Err(CodecError::UnknownTag(other)),
        };
        let prefix = Self {
            tag,
            header_bytes: u32::from_be_bytes(bytes[8..12].try_into().expect("fixed prefix")),
            payload_bytes: u32::from_be_bytes(bytes[12..16].try_into().expect("fixed prefix")),
        };
        prefix.validate(limits)?;
        Ok(prefix)
    }
    fn validate(&self, limits: &TransportLimits) -> Result<(), CodecError> {
        let header_limit = match self.tag {
            FrameTag::TransportControl => limits.max_control_bytes,
            FrameTag::BlockData | FrameTag::MetadataPart => MAX_DATA_HEADER_BYTES,
        };
        let total = u64::from(self.header_bytes) + u64::from(self.payload_bytes) + PREFIX_BYTES as u64;
        if total > u64::from(limits.max_frame_bytes) || self.header_bytes > header_limit {
            return Err(CodecError::TooLarge);
        }
        if self.header_bytes == 0
            || match self.tag {
                FrameTag::TransportControl => self.payload_bytes != 0,
                FrameTag::BlockData | FrameTag::MetadataPart => self.payload_bytes == 0,
            }
        {
            return Err(CodecError::LengthMismatch);
        }
        Ok(())
    }
    pub fn frame_bytes(&self) -> usize {
        PREFIX_BYTES + self.header_bytes as usize + self.payload_bytes as usize
    }
    fn encode(&self) -> [u8; PREFIX_BYTES] {
        let mut bytes = [0; PREFIX_BYTES];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4] = VERSION;
        bytes[5] = self.tag as u8;
        bytes[8..12].copy_from_slice(&self.header_bytes.to_be_bytes());
        bytes[12..16].copy_from_slice(&self.payload_bytes.to_be_bytes());
        bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    Control(TransportControl),
    BlockData { header: DataHeader, payload: &'a [u8] },
    MetadataPart { header: MetadataPart, payload: &'a [u8] },
}
impl Frame<'_> {
    pub fn session(&self) -> &SessionId {
        match self {
            Self::Control(control) => control.session(),
            Self::BlockData { header, .. } => &header.session,
            Self::MetadataPart { header, .. } => &header.session,
        }
    }
    /// Charges only payload bytes; headers and control use a separate budget.
    pub fn credit(&self) -> Option<(CreditStream, u32)> {
        match self {
            Self::Control(_) => None,
            Self::BlockData { header, payload } => {
                Some((CreditStream::Transfer { transfer: header.transfer.clone() }, payload.len() as u32))
            }
            Self::MetadataPart { payload, .. } => Some((CreditStream::Metadata, payload.len() as u32)),
        }
    }
}

pub fn decode<'a>(bytes: &'a [u8], limits: &TransportLimits) -> Result<Frame<'a>, CodecError> {
    let prefix = FramePrefix::decode(bytes.get(..PREFIX_BYTES).ok_or(CodecError::LengthMismatch)?, limits)?;
    if bytes.len() != prefix.frame_bytes() {
        return Err(CodecError::LengthMismatch);
    }
    let (header, payload) = bytes[PREFIX_BYTES..].split_at(prefix.header_bytes as usize);
    let frame = match prefix.tag {
        FrameTag::TransportControl => Frame::Control(parse(header)?),
        FrameTag::BlockData => Frame::BlockData { header: parse(header)?, payload },
        FrameTag::MetadataPart => Frame::MetadataPart { header: parse(header)?, payload },
    };
    validate(&frame, limits)?;
    Ok(frame)
}

pub fn encode(frame: &Frame<'_>, limits: &TransportLimits) -> Result<Vec<u8>, CodecError> {
    limits.validate().map_err(CodecError::InvalidFrame)?;
    validate(frame, limits)?;
    let (tag, header, payload) = match frame {
        Frame::Control(value) => {
            (FrameTag::TransportControl, bounded_json(value, limits.max_control_bytes as usize)?, &[][..])
        }
        Frame::BlockData { header, payload } => {
            (FrameTag::BlockData, bounded_json(header, MAX_DATA_HEADER_BYTES as usize)?, *payload)
        }
        Frame::MetadataPart { header, payload } => {
            (FrameTag::MetadataPart, bounded_json(header, MAX_DATA_HEADER_BYTES as usize)?, *payload)
        }
    };
    let prefix = FramePrefix {
        tag,
        header_bytes: header.len().try_into().map_err(|_| CodecError::TooLarge)?,
        payload_bytes: payload.len().try_into().map_err(|_| CodecError::TooLarge)?,
    };
    prefix.validate(limits)?;
    let mut out = Vec::with_capacity(prefix.frame_bytes());
    out.extend_from_slice(&prefix.encode());
    out.extend_from_slice(&header);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Available payload after serializing this header, before applying credit.
/// Call again if an offset/sequence change increases the header's decimal width.
pub fn payload_capacity(header: &impl Serialize, limits: &TransportLimits) -> Result<u32, CodecError> {
    limits.validate().map_err(CodecError::InvalidFrame)?;
    let bytes = bounded_json(header, MAX_DATA_HEADER_BYTES as usize)?;
    limits
        .max_frame_bytes
        .checked_sub((PREFIX_BYTES + bytes.len()) as u32)
        .filter(|count| *count > 0)
        .ok_or(CodecError::TooLarge)
}

fn validate(frame: &Frame<'_>, limits: &TransportLimits) -> Result<(), CodecError> {
    match frame {
        Frame::Control(value) => value.validate().map_err(CodecError::InvalidFrame),
        Frame::BlockData { header, payload } => {
            if payload.is_empty()
                || payload.len() > MAX_FRAME_BYTES as usize
                || payload.len() != header.block.payload_bytes as usize
                || header.block.blob.domain != header.block.holding.scope
                || header
                    .block
                    .offset
                    .0
                    .checked_add(payload.len() as u64)
                    .is_none_or(|end| end > u64::from(super::BLOCK_BYTES + 64 * 1024))
            {
                return Err(CodecError::InvalidFrame("invalid block payload bounds or scope".into()));
            }
            if Digest::of(payload) != header.block.frame_digest {
                return Err(CodecError::CorruptFrame);
            }
            Ok(())
        }
        Frame::MetadataPart { header, payload } => validate_metadata(header, payload.len(), limits),
    }
}
/// Validates one bounded, contiguous-on-assembly metadata fragment.
pub fn validate_metadata(header: &MetadataPart, bytes: usize, limits: &TransportLimits) -> Result<(), CodecError> {
    if header.total_bytes.0 > u64::from(limits.max_metadata_bytes) {
        return Err(CodecError::TooLarge);
    }
    if bytes == 0
        || bytes > MAX_FRAME_BYTES as usize
        || header.total_bytes.0 == 0
        || header.offset.0.checked_add(bytes as u64).is_none_or(|end| end > header.total_bytes.0)
    {
        return Err(CodecError::LengthMismatch);
    }
    Ok(())
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    // Keep serde_json's recursion bound; header byte bounds are checked first.
    serde_json::from_slice(bytes).map_err(|_| CodecError::InvalidFrame("invalid JSON header".into()))
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(io::Error::other("encoded value exceeds bound"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
/// Stops serialization at the bound rather than allocating an oversized value.
pub fn bounded_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, CodecError> {
    let mut writer = LimitedWriter { bytes: Vec::with_capacity(limit), limit };
    serde_json::to_writer(&mut writer, value).map_err(|_| CodecError::TooLarge)?;
    Ok(writer.bytes)
}
