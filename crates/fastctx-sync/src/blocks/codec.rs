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

/// Checks both identities and the decoded size. A matching compressed digest
/// alone is never evidence that the advertised content exists on this replica.
pub fn decode(representation: &Representation, encoded: Vec<u8>) -> Result<Vec<u8>, Error> {
    representation.validate().map_err(Error::Invalid)?;
    if encoded.len() != representation.encoded_bytes as usize || Digest::of(&encoded) != representation.encoded_digest {
        return Err(Error::Invalid("encoded block length or digest differs from its descriptor".into()));
    }
    let decoded = match representation.envelope.encoding {
        Encoding::Raw => encoded,
        Encoding::Zstd => {
            let mut decoder = zstd::stream::read::Decoder::new(encoded.as_slice()).map_err(codec_error)?;
            // A small output limit alone does not bound a hostile frame's window (2026-11-08).
            decoder.window_log_max(20).map_err(codec_error)?;
            let mut decoded = Vec::with_capacity(representation.content.decoded_bytes as usize);
            decoder
                .take(u64::from(representation.content.decoded_bytes) + 1)
                .read_to_end(&mut decoded)
                .map_err(codec_error)?;
            decoded
        }
    };
    if decoded.len() != representation.content.decoded_bytes as usize
        || Digest::of(&decoded) != representation.content.id.digest
    {
        return Err(Error::Invalid("decoded block length or digest differs from its content identity".into()));
    }
    Ok(decoded)
}

fn codec_error(error: std::io::Error) -> Error {
    Error::Invalid(format!("block codec rejected its input: {error}"))
}
