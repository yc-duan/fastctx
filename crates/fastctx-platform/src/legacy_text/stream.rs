//! Incremental native decoding without splitting a multibyte character.

use std::io;

use super::CodePage;

/// A strict decoder for a machine code page. Failures report the zero-based
/// byte offset within this stream. Finish with `feed(_, true)` at EOF.
pub struct LegacyDecoder {
    page: CodePage,
    max_character: usize,
    pending: Vec<u8>,
    consumed: u64,
    failed: Option<u64>,
}

impl LegacyDecoder {
    pub fn new(page: CodePage) -> io::Result<Self> {
        Ok(Self { page, max_character: width(page)?, pending: Vec::new(), consumed: 0, failed: None })
    }

    pub fn feed(&mut self, bytes: &[u8], end: bool) -> Result<String, u64> {
        if let Some(offset) = self.failed {
            return Err(offset);
        }
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(bytes);
        if let Some(text) = self.page.decode(&input) {
            self.consumed += input.len() as u64;
            return Ok(text);
        }
        if !end {
            // Only a trailing character can straddle reads. The native code
            // page supplies its maximum width; no bytes are guessed or replaced.
            for trailing in 1..self.max_character.min(input.len() + 1) {
                let complete = input.len() - trailing;
                if let Some(text) = self.page.decode(&input[..complete]) {
                    self.pending.extend_from_slice(&input[complete..]);
                    self.consumed += complete as u64;
                    return Ok(text);
                }
            }
        }
        // Conversion is normally batched. Only a failed conversion needs a
        // character walk to locate the first byte that no character consumes.
        let mut valid = 0;
        while valid < input.len() {
            let width = (1..=self.max_character.min(input.len() - valid))
                .find(|&width| self.page.decode(&input[valid..valid + width]).is_some());
            match width {
                Some(width) => valid += width,
                None => break,
            }
        }
        let offset = self.consumed + valid as u64;
        self.failed = Some(offset);
        Err(offset)
    }
}

#[cfg(windows)]
fn width(page: CodePage) -> io::Result<usize> {
    use windows_sys::Win32::Globalization::{CPINFO, GetCPInfo};
    let mut info: CPINFO = unsafe { std::mem::zeroed() };
    // SAFETY: info is writable for the complete CPINFO structure.
    if unsafe { GetCPInfo(page.0, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.MaxCharSize as usize)
}

#[cfg(not(windows))]
fn width(_page: CodePage) -> io::Result<usize> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "this machine has no legacy code pages"))
}
