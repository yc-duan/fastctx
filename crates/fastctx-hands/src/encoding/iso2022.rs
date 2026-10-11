//! State at a character boundary in a strictly validated ISO-2022-JP stream.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum State {
    #[default]
    Ascii,
    Roman,
    Katakana,
    Jis,
}

impl State {
    pub fn escape(self) -> &'static [u8] {
        match self {
            Self::Ascii => b"\x1b(B",
            Self::Roman => b"\x1b(J",
            Self::Katakana => b"\x1b(I",
            Self::Jis => b"\x1b$B",
        }
    }

    pub fn after(mut self, bytes: &[u8]) -> Self {
        for escape in bytes.windows(3) {
            self = match escape {
                b"\x1b(B" => Self::Ascii,
                b"\x1b(J" => Self::Roman,
                b"\x1b(I" => Self::Katakana,
                b"\x1b$B" | b"\x1b$@" => Self::Jis,
                _ => self,
            };
        }
        self
    }
}

pub(super) fn replacement(
    codec: super::TextEncoding,
    original: &[u8],
    range: std::ops::Range<usize>,
    text: &str,
) -> Result<Vec<u8>, super::EncodingError> {
    let before = State::Ascii.after(&original[..range.start]);
    let after = before.after(&original[range.clone()]);
    let suffix = &original[range.end..];
    let restore = !suffix.is_empty() && suffix[0] != 0x1b;
    if let Some(mut bytes) = in_state(before, text) {
        if restore && before != after {
            bytes.extend_from_slice(after.escape());
        }
        return Ok(bytes);
    }
    let mut encoded = codec.encode(text)?;
    let retained_state = encoded.starts_with(before.escape());
    if retained_state {
        encoded.drain(..before.escape().len());
    }
    let mut result = Vec::new();
    if !encoded.is_empty() && encoded[0] != 0x1b && before != State::Ascii && !retained_state {
        result.extend_from_slice(State::Ascii.escape());
    }
    // An existing suffix switch supplies its own state. Otherwise replace the
    // encoder's final ASCII reset with the state the following data needs.
    if !suffix.is_empty() && encoded.ends_with(State::Ascii.escape()) {
        encoded.truncate(encoded.len() - State::Ascii.escape().len());
    }
    result.extend_from_slice(&encoded);
    let ended = before.after(&result);
    if restore && ended != after {
        result.extend_from_slice(after.escape());
    }
    Ok(result)
}

fn in_state(state: State, text: &str) -> Option<Vec<u8>> {
    text.chars()
        .map(|c| match (state, c) {
            (State::Roman, '¥') => Some(0x5c),
            (State::Roman, '‾') => Some(0x7e),
            (State::Katakana, '｡'..='ﾟ') => Some((c as u32 - 0xff61 + 0x21) as u8),
            (State::Ascii, c) if c.is_ascii() && !matches!(c as u8, 0x0e | 0x0f | 0x1b) => Some(c as u8),
            (State::Roman, c) if c.is_ascii() && !matches!(c as u8, 0x0e | 0x0f | 0x1b | 0x5c | 0x7e) => Some(c as u8),
            _ => None,
        })
        .collect()
}

/// Half-width katakana has a lossless ISO-2022-JP state. The web encoder's
/// full-width substitution is unsuitable for file writes that promise exact text.
pub(super) fn encode(text: &str) -> Result<Vec<u8>, super::EncodingError> {
    let mut result = Vec::new();
    let mut normal = String::new();
    for c in text.chars() {
        if ('｡'..='ﾟ').contains(&c) {
            if !normal.is_empty() {
                append(&mut result, &super::encode::legacy(encoding_rs::ISO_2022_JP, &normal)?);
                normal.clear();
            }
            let mut katakana = State::Katakana.escape().to_vec();
            katakana.push((c as u32 - 0xff61 + 0x21) as u8);
            katakana.extend_from_slice(State::Ascii.escape());
            append(&mut result, &katakana);
        } else {
            normal.push(c);
        }
    }
    if !normal.is_empty() {
        append(&mut result, &super::encode::legacy(encoding_rs::ISO_2022_JP, &normal)?);
    }
    Ok(result)
}

fn append(bytes: &mut Vec<u8>, next: &[u8]) {
    // Only switches newly produced by the encoder are coalesced. Original
    // file switches outside a replacement never pass through this function.
    if next.starts_with(&[0x1b]) && bytes.ends_with(State::Ascii.escape()) {
        bytes.truncate(bytes.len() - State::Ascii.escape().len());
    }
    bytes.extend_from_slice(next);
}

pub(super) fn has_switch(bytes: &[u8]) -> bool {
    bytes.windows(3).any(|bytes| matches!(bytes, b"\x1b$B" | b"\x1b$@" | b"\x1b(B" | b"\x1b(J" | b"\x1b(I"))
}
