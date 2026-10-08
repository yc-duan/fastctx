use crate::encoding::{self, TextEncoding};

pub(super) struct Document {
    pub text: String,
    encoding: TextEncoding,
    bom: Vec<u8>,
    pub eol: &'static str,
}

impl Document {
    pub fn label(&self) -> &'static str {
        self.encoding.label()
    }

    pub fn decode(raw: &[u8], label: Option<&str>) -> Result<Self, String> {
        let source = encoding::file::Source::decode(raw.to_vec(), label)?;
        let label = source.label().ok_or_else(|| {
            "apply_patch needs one resolved encoding; use edit within resolved segments of a mixed file".to_string()
        })?;
        let encoding = encoding::explicit(label, raw).map_err(|error| error.to_string())?;
        let text = encoding.decode_for_write(raw).map_err(|e| e.to_string())?;
        let eol = crate::text::dominant_ending(&text);
        Ok(Self { text, encoding, bom: raw[..encoding.bom_len()].to_vec(), eol })
    }

    pub fn newlines(&self, text: &str) -> String {
        crate::text::newlines(text, self.eol)
    }

    pub fn encode(&self, text: &str) -> Result<Vec<u8>, String> {
        let mut bytes = self.bom.clone();
        bytes.extend(self.encoding.encode(text).map_err(|e| e.to_string())?);
        Ok(bytes)
    }

    pub fn replace(&self, text: &str) -> Result<Vec<u8>, String> {
        self.encode(&self.newlines(text))
    }
}
