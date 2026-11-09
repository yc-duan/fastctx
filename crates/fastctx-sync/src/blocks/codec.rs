//! Block codecs validate representation bytes before accepting decoded content.

use crate::{
    identity::{Digest, ScopeKey},
    manifest::{BlobId, ContentBlock, DigestAlgorithm, Encoding, Envelope, Protection, Representation},
    store::Error,
};
use fastctx_types::sync::BLOCK_BYTES;
use std::io::{Read, Write};

pub struct EncodedBlock {
    pub representation: Representation,
    pub bytes: Vec<u8>,
}

/// Encodes one bounded block outside the database writer. Compression changes
/// the representation, never the content identity or its authorization domain.
pub fn encode(scope: ScopeKey, decoded: Vec<u8>, encoding: Encoding) -> Result<EncodedBlock, Error> {
    if decoded.len() > BLOCK_BYTES as usize {
        return Err(Error::Invalid("a content block exceeds one MiB".into()));
    }
    let content = ContentBlock {
        id: BlobId { domain: scope, algorithm: DigestAlgorithm::Sha256, digest: Digest::of(&decoded) },
        decoded_bytes: decoded.len() as u32,
    };
    let bytes = match encoding {
        Encoding::Raw => decoded,
        Encoding::Zstd => {
            let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).map_err(codec_error)?;
            encoder.window_log(20).map_err(codec_error)?;
            encoder.write_all(&decoded).map_err(codec_error)?;
            encoder.finish().map_err(codec_error)?
        }
    };
    let representation = Representation {
        content,
        encoded_bytes: u32::try_from(bytes.len()).map_err(|_| Error::Invalid("encoded block is too large".into()))?,
        encoded_digest: Digest::of(&bytes),
        envelope: Envelope { version: 1, encoding, protection: Protection::Plain },
    };
    representation.validate().map_err(Error::Invalid)?;
    Ok(EncodedBlock { representation, bytes })
}

/// Decoder state, its one-MiB window, native input/output buffers and the
/// 64-KiB verification chunk. Callers charge this in addition to retained bytes.
/// Zstd 1.5.7 estimates DCtx + window + three 128-KiB blocks + 64 bytes;
/// this bound also includes the Rust verification chunk, with no BufReader copy.
pub const fn decode_workspace(encoding: Encoding) -> usize {
    match encoding {
        Encoding::Raw => 0,
        Encoding::Zstd => 2 * 1024 * 1024,
    }
}

/// Checks content without keeping a decoded copy or cloning the input block.
pub fn validate(representation: &Representation, encoded: &[u8]) -> Result<(), Error> {
    visit(representation, encoded, |_| {})
}

/// Returns bounded decoded bytes; callers also reserve decode_workspace and
/// the encoded input. Raw content reuses its input allocation.
pub fn decode(representation: &Representation, encoded: Vec<u8>) -> Result<Vec<u8>, Error> {
    if representation.envelope.encoding == Encoding::Raw {
        validate(representation, &encoded)?;
        return Ok(encoded);
    }
    let mut decoded = Vec::with_capacity(representation.content.decoded_bytes as usize);
    visit(representation, &encoded, |bytes| decoded.extend_from_slice(bytes))?;
    Ok(decoded)
}

fn visit(representation: &Representation, encoded: &[u8], mut consume: impl FnMut(&[u8])) -> Result<(), Error> {
    use sha2::{Digest as _, Sha256};
    representation.validate().map_err(Error::Invalid)?;
    if encoded.len() != representation.encoded_bytes as usize || Digest::of(encoded) != representation.encoded_digest {
        return Err(Error::Invalid("encoded block length or digest differs from its descriptor".into()));
    }
    if representation.envelope.encoding == Encoding::Raw {
        if encoded.len() != representation.content.decoded_bytes as usize
            || Digest::of(encoded) != representation.content.id.digest
        {
            return Err(Error::Invalid("raw block differs from its content identity".into()));
        }
        consume(encoded);
        return Ok(());
    }
    let mut decoder = zstd::stream::read::Decoder::with_buffer(encoded).map_err(codec_error)?;
    decoder.window_log_max(20).map_err(codec_error)?;
    let mut chunk = [0_u8; 64 * 1024];
    let mut hash = Sha256::new();
    let mut total = 0_usize;
    loop {
        let size = decoder.read(&mut chunk).map_err(codec_error)?;
        if size == 0 {
            break;
        }
        total = total.checked_add(size).ok_or_else(|| Error::Invalid("decoded block size overflow".into()))?;
        if total > representation.content.decoded_bytes as usize {
            return Err(Error::Invalid("decoded block exceeds its declared length".into()));
        }
        hash.update(&chunk[..size]);
        consume(&chunk[..size]);
    }
    if total != representation.content.decoded_bytes as usize
        || Digest::from_bytes(hash.finalize().into()) != representation.content.id.digest
    {
        return Err(Error::Invalid("decoded block length or digest differs from its content identity".into()));
    }
    Ok(())
}

fn codec_error(error: std::io::Error) -> Error {
    Error::Invalid(format!("block codec rejected its input: {error}"))
}
