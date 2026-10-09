//! Replayable bytes with an answer-sized in-memory prefix and anonymous spill files.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::sync::Arc;

pub(crate) const BLOCK: usize = 64 * 1024;
pub(crate) const MEMORY: usize = crate::run::window::bytes(crate::run::window::BUDGET);

#[derive(Debug)]
enum Storage {
    Memory(Vec<u8>),
    File { file: File, written: u64, tail: Vec<u8> },
}

#[derive(Debug, Clone)]
pub(crate) struct Bytes {
    storage: Arc<Storage>,
    range: Range<u64>,
}

pub(crate) struct Spool {
    storage: Storage,
    length: u64,
}

impl Default for Spool {
    fn default() -> Self {
        Self { storage: Storage::Memory(Vec::new()), length: 0 }
    }
}

impl Spool {
    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn as_slice(&self) -> Option<&[u8]> {
        match &self.storage {
            Storage::Memory(bytes) => Some(bytes),
            Storage::File { .. } => None,
        }
    }

    pub fn clear(&mut self) {
        match &mut self.storage {
            Storage::Memory(bytes) => bytes.clear(),
            Storage::File { .. } => self.storage = Storage::Memory(Vec::new()),
        }
        self.length = 0;
    }

    pub fn finish(self) -> Bytes {
        Bytes { storage: Arc::new(self.storage), range: 0..self.length }
    }
}

impl Write for Spool {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if matches!(&self.storage, Storage::Memory(buffer) if buffer.len().saturating_add(bytes.len()) > MEMORY) {
            let mut file = tempfile::tempfile()?;
            if let Storage::Memory(buffer) = &self.storage {
                file.write_all(buffer)?;
            }
            self.storage = Storage::File { file, written: self.length, tail: Vec::with_capacity(BLOCK) };
        }
        match &mut self.storage {
            Storage::Memory(buffer) => {
                let needed = buffer.len() + bytes.len();
                if needed > buffer.capacity() {
                    let capacity = needed.max(buffer.capacity().saturating_mul(2)).clamp(8, MEMORY);
                    buffer.reserve_exact(capacity - buffer.len());
                }
                buffer.extend_from_slice(bytes);
            }
            Storage::File { file, written, tail } => {
                let mut bytes = bytes;
                while !bytes.is_empty() {
                    let count = (BLOCK - tail.len()).min(bytes.len());
                    tail.extend_from_slice(&bytes[..count]);
                    bytes = &bytes[count..];
                    if tail.len() == BLOCK {
                        file.write_all(tail)?;
                        *written += tail.len() as u64;
                        tail.clear();
                    }
                }
            }
        }
        self.length =
            self.length.checked_add(bytes.len() as u64).ok_or_else(|| io::Error::other("spool length overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Bytes {
    pub fn as_slice(&self) -> Option<&[u8]> {
        match self.storage.as_ref() {
            Storage::Memory(bytes) => Some(&bytes[self.range.start as usize..self.range.end as usize]),
            Storage::File { .. } => None,
        }
    }

    pub fn len(&self) -> usize {
        usize::try_from(self.range.end - self.range.start).expect("addressable input")
    }
    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }
    pub fn slice(&self, range: Range<usize>) -> Self {
        assert!(range.start <= range.end && range.end <= self.len());
        Self {
            storage: self.storage.clone(),
            range: self.range.start + range.start as u64..self.range.start + range.end as u64,
        }
    }
    pub fn reader(&self) -> Reader {
        Reader { bytes: self.clone(), position: 0 }
    }
    pub fn chunks(&self) -> Chunks {
        Chunks(self.reader())
    }
    pub fn to_vec(&self) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.as_slice() {
            return Ok(bytes.to_vec());
        }
        let mut bytes = Vec::new();
        self.reader().read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    pub fn text(&self) -> io::Result<String> {
        String::from_utf8(self.to_vec()?).map_err(io::Error::other)
    }
    pub fn byte(&self, at: usize) -> io::Result<Option<u8>> {
        if at >= self.len() {
            return Ok(None);
        }
        let mut byte = [0];
        self.slice(at..at + 1).reader().read_exact(&mut byte)?;
        Ok(Some(byte[0]))
    }
    pub fn ends_with(&self, suffix: &[u8]) -> io::Result<bool> {
        Ok(self.len() >= suffix.len() && self.slice(self.len() - suffix.len()..self.len()).to_vec()? == suffix)
    }
    pub fn is_ascii(&self) -> io::Result<bool> {
        if let Some(bytes) = self.as_slice() {
            return Ok(bytes.is_ascii());
        }
        for chunk in self.chunks() {
            if !chunk?.is_ascii() {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn equals(&self, other: &Self) -> io::Result<bool> {
        if self.len() != other.len() {
            return Ok(false);
        }
        let mut a = self.reader();
        let mut b = other.reader();
        let mut left = [0; BLOCK];
        let mut right = [0; BLOCK];
        loop {
            let n = a.read(&mut left)?;
            b.read_exact(&mut right[..n])?;
            if left[..n] != right[..n] {
                return Ok(false);
            }
            if n == 0 {
                return Ok(true);
            }
        }
    }
}

pub(crate) struct Reader {
    bytes: Bytes,
    position: u64,
}
impl Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let start = self.bytes.range.start + self.position;
        let count = out.len().min((self.bytes.range.end - start) as usize);
        if count == 0 {
            return Ok(0);
        }
        let n = match self.bytes.storage.as_ref() {
            Storage::Memory(bytes) => {
                out[..count].copy_from_slice(&bytes[start as usize..start as usize + count]);
                count
            }
            Storage::File { file, written, tail } => {
                if start >= *written {
                    let at = (start - *written) as usize;
                    out[..count].copy_from_slice(&tail[at..at + count]);
                    count
                } else {
                    let count = count.min((*written - start) as usize);
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::FileExt;
                        file.seek_read(&mut out[..count], start)?
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::FileExt;
                        file.read_at(&mut out[..count], start)?
                    }
                }
            }
        };
        self.position += n as u64;
        Ok(n)
    }
}
impl Seek for Reader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let at = match from {
            SeekFrom::Start(at) => i128::from(at),
            SeekFrom::End(delta) => self.bytes.len() as i128 + i128::from(delta),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
        };
        if at < 0 || at > self.bytes.len() as i128 {
            return Err(io::Error::other("seek outside spool"));
        }
        self.position = at as u64;
        Ok(self.position)
    }
}

pub(crate) struct Chunks(Reader);
impl Iterator for Chunks {
    type Item = io::Result<Vec<u8>>;
    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.0.bytes.len() - self.0.position as usize;
        if remaining == 0 {
            return None;
        }
        let mut buffer = vec![0; remaining.min(BLOCK)];
        match self.0.read(&mut buffer) {
            Ok(0) => None,
            Ok(n) => {
                buffer.truncate(n);
                Some(Ok(buffer))
            }
            Err(e) => Some(Err(e)),
        }
    }
}

impl Bytes {
    pub fn utf8(&self, mut consume: impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        if let Some(bytes) = self.as_slice()
            && let Ok(text) = std::str::from_utf8(bytes)
        {
            return consume(text);
        }
        let mut decoder = crate::encoding::TextDecoder::new(crate::encoding::TextEncoding::utf8());
        for bytes in self.chunks() {
            consume(&decoder.feed(&bytes?, false).map_err(io::Error::other)?)?;
        }
        consume(&decoder.feed(&[], true).map_err(io::Error::other)?)
    }
    pub fn chars(&self) -> Chars {
        Chars {
            chunks: self.chunks(),
            decoder: crate::encoding::TextDecoder::new(crate::encoding::TextEncoding::utf8()),
            text: String::new(),
            at: 0,
            ended: false,
        }
    }
}

pub(crate) struct Chars {
    chunks: Chunks,
    decoder: crate::encoding::TextDecoder,
    text: String,
    at: usize,
    ended: bool,
}
impl Iterator for Chars {
    type Item = io::Result<char>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(c) = self.text[self.at..].chars().next() {
                self.at += c.len_utf8();
                return Some(Ok(c));
            }
            if self.ended {
                return None;
            }
            let bytes = match self.chunks.next() {
                Some(Ok(bytes)) => bytes,
                Some(Err(error)) => {
                    self.ended = true;
                    return Some(Err(error));
                }
                None => {
                    self.ended = true;
                    Vec::new()
                }
            };
            match self.decoder.feed(&bytes, self.ended) {
                Ok(text) => {
                    self.text = text;
                    self.at = 0;
                }
                Err(error) => {
                    self.ended = true;
                    return Some(Err(io::Error::other(error)));
                }
            }
        }
    }
}
