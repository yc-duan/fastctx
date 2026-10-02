//! Encoding-preserving transformations of a frozen byte snapshot.

use super::WriteContext;
use crate::encoding::{
    ByteSource, EncodingDecision, ValidatedFileEncoding, validate_source_encoding,
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
                return Err(reason.message(&path.to_string_lossy()));
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
