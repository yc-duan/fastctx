//! Encoding-preserving transformations of a frozen byte snapshot.

use super::WriteContext;
use crate::encoding::{
    ByteSource, EncodingDecision, EncodingRejection, ValidatedFileEncoding,
    validate_source_encoding,
};
use std::path::Path;

pub(super) struct Document {
    pub text: String,
    encoding: ValidatedFileEncoding,
    bom: Vec<u8>,
    pub eol: &'static str,
}

impl Document {
    pub fn decode(raw: &[u8], path: &Path, context: &WriteContext<'_>) -> Result<Self, String> {
        let explicit = context.source_encoding.and_then(|get| get(path));
        let encoding = match validate_source_encoding(ByteSource::Bytes(raw), explicit.as_deref())
            .map_err(|error| error.to_string())?
        {
            EncodingDecision::Text(encoding) => encoding,
            EncodingDecision::Binary => return Err("Cannot write a binary file as text.".into()),
            EncodingDecision::Rejected(reason) => {
                return Err(rejection(&reason, &path.to_string_lossy()));
            }
        };
        let text = encoding.decode_editable_snapshot(raw)?;
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - crlf;
        let cr = text.matches('\r').count() - crlf;
        let eol = if crlf > lf && crlf >= cr {
            "\r\n"
        } else if cr > lf {
            "\r"
        } else {
            "\n"
        };
        let bom = raw[..encoding.editable_raw_start()].to_vec();
        Ok(Self {
            text,
            encoding,
            bom,
            eol,
        })
    }

    pub fn newlines(&self, text: &str) -> String {
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', self.eol)
    }

    pub fn encode(&self, text: &str) -> Result<Vec<u8>, String> {
        let bytes = self.encoding.encode_fragment(text).ok_or_else(|| format!(
            "Replacement text cannot be encoded as {}. Use representable text or convert the file to UTF-8.",
            self.encoding.encoding_label()
        ))?;
        let mut result = self.bom.clone();
        result.extend(bytes);
        Ok(result)
    }

    pub fn encode_replacement(&self, text: &str) -> Result<Vec<u8>, String> {
        self.encode(&self.newlines(text))
    }
}

/// Why a file's text cannot be settled, in the terms of the write tools: they take no encoding
/// argument and write in the encoding the file was last viewed in, so the way out is a view.
fn rejection(reason: &EncodingRejection, path: &str) -> String {
    const VIEWED: &str = "Writes then use the encoding it was viewed in.";
    match reason {
        EncodingRejection::Ambiguous { candidates } => format!(
            "Cannot determine the text encoding of {path} with confidence: the bytes decode cleanly as {}. View it with `fastctx view --encoding <label>` naming the one the context tells you. {VIEWED}",
            candidates.join(", ")
        ),
        EncodingRejection::Iso2022JpSignature => format!(
            "Cannot decode {path} as text with confidence: the bytes are valid UTF-8 but contain ISO-2022 escape sequences. View it with `fastctx view --encoding iso-2022-jp`, or `--encoding utf-8` for the raw UTF-8 reading. {VIEWED}"
        ),
        EncodingRejection::ExplicitMalformed { encoding } => format!(
            "Cannot decode {path} as {encoding}, the encoding it was last viewed in: it is no longer valid {encoding}. View it again, with `--encoding` if it needs another one. {VIEWED}"
        ),
        EncodingRejection::MixedOrInconsistent {
            conflict_hex_offset,
        } => {
            // The rejection names a one-based row of a 16-byte hex dump; `xxd` labels rows by
            // their first byte.
            let conflict = conflict_hex_offset.map_or_else(String::new, |row| {
                let start = (row - 1) * 16;
                format!(" The first conflicting bytes are in the 16 bytes from offset {start} (0x{start:08x} in `xxd`).")
            });
            format!(
                "Cannot write {path} as text: it appears to mix encodings; no single encoding explains the whole file.{conflict} Inspect the raw bytes with `xxd`, or convert the file to one encoding first."
            )
        }
        EncodingRejection::Undecodable => {
            format!(
                "Cannot write {path} as text: no supported encoding decodes it cleanly. Inspect its raw bytes with `xxd`."
            )
        }
        EncodingRejection::BomMismatch { encoding } => format!(
            "Cannot write {path}: it has a {encoding} byte order mark but the content is not valid {encoding}. Inspect its raw bytes with `xxd`."
        ),
        EncodingRejection::InvalidLabel { .. } => reason.message(path),
    }
}
