use super::EncodingError;
use encoding_rs::Encoding;

pub(super) fn legacy(encoding: &'static Encoding, text: &str) -> Result<Vec<u8>, EncodingError> {
    let mut encoder = encoding.new_encoder();
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let capacity = encoder
            .max_buffer_length_from_utf8_without_replacement(text.len() - offset)
            .expect("encoding buffer length overflow");
        bytes.reserve(capacity);
        let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(&text[offset..], &mut bytes, true);
        offset += read;
        match result {
            encoding_rs::EncoderResult::InputEmpty => return Ok(bytes),
            encoding_rs::EncoderResult::OutputFull => continue,
            encoding_rs::EncoderResult::Unmappable(character) => {
                return Err(EncodingError::Unrepresentable { encoding: encoding.name().into(), character });
            }
        }
    }
}
