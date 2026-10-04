use super::{Codec, EncodingError, TextEncoding};

enum Decoder {
    Rs(encoding_rs::Decoder),
    Utf32 { little: bool, pending: Vec<u8> },
}

/// Incremental strict decoding, including split BOMs and split code points.
/// `feed(_, true)` is required at EOF. On error no text from that call is returned;
/// the decoder remains failed. Offsets are zero-based original-file byte offsets.
pub struct TextDecoder {
    encoding: TextEncoding,
    decoder: Decoder,
    skip: usize,
    consumed: usize,
    failed: Option<EncodingError>,
    finished: bool,
}

impl TextDecoder {
    pub fn new(encoding: TextEncoding) -> Self {
        let decoder = match encoding.codec {
            Codec::Rs(enc) => Decoder::Rs(enc.new_decoder_without_bom_handling()),
            Codec::Utf32Le => Decoder::Utf32 { little: true, pending: Vec::new() },
            Codec::Utf32Be => Decoder::Utf32 { little: false, pending: Vec::new() },
        };
        Self { encoding, decoder, skip: encoding.bom_len(), consumed: 0, failed: None, finished: false }
    }

    pub fn feed(&mut self, bytes: &[u8], end: bool) -> Result<String, EncodingError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        assert!(!self.finished, "decoder fed after end of input");
        self.finished = end;
        let skip = bytes.len().min(self.skip);
        self.skip -= skip;
        self.consumed += skip;
        let bytes = &bytes[skip..];
        let result = self.decode(bytes, end);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    fn decode(&mut self, bytes: &[u8], end: bool) -> Result<String, EncodingError> {
        let label = self.encoding.label().to_string();
        if end && self.skip != 0 {
            return Err(EncodingError::Malformed { encoding: label, offset: self.consumed });
        }
        match &mut self.decoder {
            Decoder::Rs(decoder) => {
                let mut output = String::new();
                let mut offset = 0;
                loop {
                    let size = decoder
                        .max_utf8_buffer_length_without_replacement(bytes.len() - offset)
                        .expect("decoding buffer length overflow");
                    output.reserve(size);
                    let (result, read) =
                        decoder.decode_to_string_without_replacement(&bytes[offset..], &mut output, end);
                    offset += read;
                    self.consumed += read;
                    match result {
                        encoding_rs::DecoderResult::InputEmpty => return Ok(output),
                        encoding_rs::DecoderResult::OutputFull => continue,
                        encoding_rs::DecoderResult::Malformed(length, after) => {
                            return Err(EncodingError::Malformed {
                                encoding: label,
                                offset: self.consumed - usize::from(length) - usize::from(after),
                            });
                        }
                    }
                }
            }
            Decoder::Utf32 { little, pending } => {
                let start = self.consumed - pending.len();
                self.consumed += bytes.len();
                pending.extend_from_slice(bytes);
                let complete = pending.len() / 4 * 4;
                let mut text = String::new();
                for (i, raw) in pending[..complete].as_chunks::<4>().0.iter().enumerate() {
                    let raw = *raw;
                    let unit = if *little { u32::from_le_bytes(raw) } else { u32::from_be_bytes(raw) };
                    text.push(
                        char::from_u32(unit).ok_or_else(|| EncodingError::Malformed {
                            encoding: label.clone(),
                            offset: start + i * 4,
                        })?,
                    );
                }
                pending.drain(..complete);
                if end && !pending.is_empty() {
                    return Err(EncodingError::Malformed { encoding: label, offset: start + complete });
                }
                Ok(text)
            }
        }
    }
}
