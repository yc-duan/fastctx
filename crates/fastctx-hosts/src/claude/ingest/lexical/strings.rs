use super::{Error, Event, Step};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Utf8 {
    #[default]
    Complete,
    One,
    Two,
    Three,
    E0,
    Ed,
    F0,
    F4,
}
impl Utf8 {
    fn byte(self, byte: u8) -> Option<Self> {
        use Utf8::*;
        Some(match (self, byte) {
            (Complete, 0..=0x7f) => Complete,
            (Complete, 0xc2..=0xdf) => One,
            (Complete, 0xe0) => E0,
            (Complete, 0xe1..=0xec | 0xee..=0xef) => Two,
            (Complete, 0xed) => Ed,
            (Complete, 0xf0) => F0,
            (Complete, 0xf1..=0xf3) => Three,
            (Complete, 0xf4) => F4,
            (One, 0x80..=0xbf) => Complete,
            (Two, 0x80..=0xbf) | (E0, 0xa0..=0xbf) | (Ed, 0x80..=0x9f) => One,
            (Three, 0x80..=0xbf) | (F0, 0x90..=0xbf) | (F4, 0x80..=0x8f) => Two,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    #[default]
    Text,
    Escape,
    Unicode {
        digits: u8,
        value: u16,
        high: Option<u16>,
    },
    SurrogateSlash {
        high: u16,
    },
    SurrogateU {
        high: u16,
    },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StringState {
    phase: Phase,
    utf8: Utf8,
}

impl StringState {
    pub(super) fn step<'a>(&mut self, input: &'a [u8], key: bool) -> Result<Step<'a>, Error> {
        let mut used = 0;
        while let Some(&byte) = input.get(used) {
            match self.phase {
                Phase::Text => {
                    let start = used;
                    while let Some(&byte) = input.get(used) {
                        if matches!(byte, b'"' | b'\\') {
                            if self.utf8 != Utf8::Complete {
                                return Err(Error::at(used, "incomplete UTF-8 before an escape or quote"));
                            }
                            break;
                        }
                        if byte < 0x20 {
                            return Err(Error::at(used, "unescaped control character"));
                        }
                        self.utf8 = self.utf8.byte(byte).ok_or_else(|| Error::at(used, "invalid UTF-8"))?;
                        used += 1;
                    }
                    if used > start {
                        return Ok(Step { used, event: Some(Event::StringBytes(&input[start..used])) });
                    }
                    if byte == b'"' {
                        return Ok(Step { used: used + 1, event: Some(Event::EndString { key }) });
                    }
                    self.phase = Phase::Escape;
                    used += 1;
                }
                Phase::Escape => {
                    used += 1;
                    if byte == b'u' {
                        self.phase = Phase::Unicode { digits: 0, value: 0, high: None };
                        continue;
                    }
                    let decoded = match byte {
                        b'"' | b'\\' | b'/' => byte,
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => 10,
                        b'r' => 13,
                        b't' => 9,
                        _ => return Err(Error::at(used - 1, "invalid JSON escape")),
                    };
                    self.phase = Phase::Text;
                    return Ok(Step { used, event: Some(Event::Escaped { bytes: [decoded, 0, 0, 0], len: 1 }) });
                }
                Phase::Unicode { digits, value, high } => {
                    let hex = match byte {
                        b'0'..=b'9' => byte - b'0',
                        b'a'..=b'f' => byte - b'a' + 10,
                        b'A'..=b'F' => byte - b'A' + 10,
                        _ => return Err(Error::at(used, "invalid Unicode escape")),
                    };
                    if digits >= 4 {
                        return Err(Error::at(used, "invalid saved Unicode state"));
                    }
                    let value = (value << 4) | u16::from(hex);
                    used += 1;
                    if digits < 3 {
                        self.phase = Phase::Unicode { digits: digits + 1, value, high };
                        continue;
                    }
                    let point = match (high, value) {
                        (None, 0xd800..=0xdbff) => {
                            self.phase = Phase::SurrogateSlash { high: value };
                            continue;
                        }
                        (Some(high @ 0xd800..=0xdbff), low @ 0xdc00..=0xdfff) => {
                            0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(low) - 0xdc00
                        }
                        (Some(_), _) | (None, 0xdc00..=0xdfff) => {
                            return Err(Error::at(used - 1, "unpaired Unicode surrogate"));
                        }
                        (None, value) => u32::from(value),
                    };
                    let mut bytes = [0; 4];
                    let len = char::from_u32(point)
                        .ok_or_else(|| Error::at(used - 1, "invalid Unicode scalar"))?
                        .encode_utf8(&mut bytes)
                        .len() as u8;
                    self.phase = Phase::Text;
                    return Ok(Step { used, event: Some(Event::Escaped { bytes, len }) });
                }
                Phase::SurrogateSlash { high } => {
                    if byte != b'\\' {
                        return Err(Error::at(used, "missing low Unicode surrogate"));
                    }
                    self.phase = Phase::SurrogateU { high };
                    used += 1;
                }
                Phase::SurrogateU { high } => {
                    if byte != b'u' {
                        return Err(Error::at(used, "missing low Unicode surrogate"));
                    }
                    self.phase = Phase::Unicode { digits: 0, value: 0, high: Some(high) };
                    used += 1;
                }
            }
        }
        Ok(Step { used, event: None })
    }
}
