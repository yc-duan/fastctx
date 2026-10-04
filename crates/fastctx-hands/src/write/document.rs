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
        let encoding = encoding::detect(raw, label).map_err(|e| e.to_string())?;
        let text = encoding.decode_for_write(raw).map_err(|e| e.to_string())?;
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
        Ok(Self { text, encoding, bom: raw[..encoding.bom_len()].to_vec(), eol })
    }

    pub fn newlines(&self, text: &str) -> String {
        text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', self.eol)
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
