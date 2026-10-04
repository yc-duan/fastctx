//! Assembles Unicode scalars across terminal reads before interpreting VT syntax.

use std::io;

use super::{Input, decode::Decoder};

#[derive(Default)]
pub(super) struct Utf8 {
    // A Unicode scalar occupies at most four bytes in UTF-8.
    bytes: [u8; 4],
    length: usize,
    expected: usize,
}

impl Utf8 {
    pub fn pending(&self) -> bool {
        self.length != 0
    }

    pub fn push(&mut self, bytes: &[u8], decoder: &mut Decoder, out: &mut Vec<Input>) -> io::Result<()> {
        for &byte in bytes {
            if self.length == 0 {
                if byte < 0x80 {
                    decoder.char(char::from(byte), out);
                    continue;
                }
                self.expected = match byte {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => return Err(invalid()),
                };
            } else if byte & 0xc0 != 0x80 {
                return Err(invalid());
            }
            self.bytes[self.length] = byte;
            self.length += 1;
            if self.length == self.expected {
                let text = std::str::from_utf8(&self.bytes[..self.length]).map_err(|_| invalid())?;
                decoder.char(text.chars().next().unwrap(), out);
                self.length = 0;
            }
        }
        Ok(())
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "terminal supplied invalid UTF-8")
}
