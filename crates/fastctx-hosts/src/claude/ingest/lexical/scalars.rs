//! Restartable scalar syntax; no token bytes are retained.
use super::{Error, Event, Step};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Literal {
    True,
    False,
    Null,
}
impl Literal {
    pub(super) fn bytes(self) -> &'static [u8] {
        match self {
            Self::True => b"true",
            Self::False => b"false",
            Self::Null => b"null",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Number {
    Start,
    Minus,
    Zero,
    Integer,
    Dot,
    Fraction,
    E,
    Sign,
    Exponent,
}
impl Number {
    pub(super) fn complete(self) -> bool {
        matches!(self, Self::Zero | Self::Integer | Self::Fraction | Self::Exponent)
    }
    fn byte(self, byte: u8) -> Result<Option<Self>, ()> {
        use Number::*;
        Ok(Some(match (self, byte) {
            (Start, b'-') => Minus,
            (Start | Minus, b'0') => Zero,
            (Start | Minus, b'1'..=b'9') => Integer,
            (Integer, b'0'..=b'9') => Integer,
            (Zero | Integer, b'.') => Dot,
            (Dot | Fraction, b'0'..=b'9') => Fraction,
            (Zero | Integer | Fraction, b'e' | b'E') => E,
            (E, b'+' | b'-') => Sign,
            (E | Sign | Exponent, b'0'..=b'9') => Exponent,
            (_, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}') if self.complete() => return Ok(None),
            _ => return Err(()),
        }))
    }
}
impl Number {
    pub(super) fn step<'a>(&mut self, input: &'a [u8]) -> Result<Step<'a>, Error> {
        let mut used = 0;
        while let Some(&byte) = input.get(used) {
            match self.byte(byte).map_err(|_| Error::at(used, "invalid JSON number"))? {
                Some(next) => {
                    *self = next;
                    used += 1;
                }
                None => break,
            }
        }
        let event = if used > 0 {
            Some(Event::NumberBytes(&input[..used]))
        } else if !input.is_empty() {
            Some(Event::EndNumber)
        } else {
            None
        };
        Ok(Step { used, event })
    }
}
impl Literal {
    pub(super) fn step<'a>(self, read: &mut u8, input: &'a [u8]) -> Result<Step<'a>, Error> {
        let mut used = 0;
        while (*read as usize) < self.bytes().len() {
            let Some(&byte) = input.get(used) else {
                return Ok(Step { used, event: None });
            };
            if byte != self.bytes()[*read as usize] {
                return Err(Error::at(used, "invalid JSON literal"));
            }
            used += 1;
            *read += 1;
        }
        Ok(Step {
            used,
            event: Some(match self {
                Self::True => Event::Bool(true),
                Self::False => Event::Bool(false),
                Self::Null => Event::Null,
            }),
        })
    }
}
