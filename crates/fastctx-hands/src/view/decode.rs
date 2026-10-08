//! Hash the same byte stream that the shared file decoder interprets.

use sha2::{Digest, Sha256};
use std::io::{self, Read};

pub(super) struct Hashed<R> {
    pub reader: R,
    pub hash: Sha256,
}

impl<R: Read> Read for Hashed<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.read(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
}
