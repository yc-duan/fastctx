//! Exact bytes can bind a native Claude picture to an upload receipt.

use std::io::{self, Read, Write};

use sha2::{Digest as _, Sha256};

#[derive(Default)]
pub struct Digest {
    bytes: u64,
    sha256: Sha256,
}

impl Digest {
    pub fn finish(self) -> (u64, String) {
        (self.bytes, format!("{:x}", self.sha256.finalize()))
    }
}

impl Write for Digest {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes += bytes.len() as u64;
        self.sha256.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn inline(data: &str) -> io::Result<(u64, String)> {
    let mut decoded = base64::read::DecoderReader::new(data.as_bytes(), &base64::engine::general_purpose::STANDARD);
    let mut digest = Digest::default();
    let mut block = [0; 8192];
    loop {
        let bytes = decoded.read(&mut block)?;
        if bytes == 0 {
            break;
        }
        digest.write_all(&block[..bytes])?;
    }
    Ok(digest.finish())
}
