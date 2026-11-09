//! Strict standard base64 with a restartable quartet and terminal-padding state.
//! Invalid image data stays available through the original JSON string; it must
//! never be published as a successfully decoded picture.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base64State {
    quartet: [u8; 4],
    used: u8,
    ended: bool,
    failed: bool,
}
impl Base64State {
    pub fn decode(&mut self, input: &[u8]) -> io::Result<Vec<u8>> {
        if input.len() > super::lexical::WINDOW_BYTES || self.failed || self.used > 3 {
            return Err(io::Error::other("invalid base64 decoding state or input window"));
        }
        let mut output = Vec::with_capacity(input.len().div_ceil(4) * 3);
        for &byte in input {
            if self.ended {
                self.failed = true;
                return Err(io::Error::other("base64 bytes follow terminal padding"));
            }
            self.quartet[self.used as usize] = byte;
            self.used += 1;
            if self.used == 4 {
                let mut decoded = [0; 3];
                let length = match base64::engine::general_purpose::STANDARD.decode_slice(self.quartet, &mut decoded) {
                    Ok(length) => length,
                    Err(error) => {
                        self.failed = true;
                        return Err(io::Error::other(error));
                    }
                };
                output.extend_from_slice(&decoded[..length]);
                self.ended = length < 3;
                self.used = 0;
            }
        }
        Ok(output)
    }
    pub fn finish(&self) -> io::Result<()> {
        if self.failed || self.used != 0 {
            return Err(io::Error::other("incomplete or invalid base64 image"));
        }
        Ok(())
    }
}
