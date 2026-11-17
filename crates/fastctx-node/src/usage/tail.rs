//! A day's file of the ledger read on from where it was last read. The
//! ledger only ever appends to a day's file, under the book's lock
//! (`ledger::append`, which cuts a torn last line before it writes), so what
//! was read stays as it was; a file that is not the one read before (shorter,
//! or other first bytes) is read again whole. A line that is no row is set
//! aside as the ledger sets it aside (`quarantine`), and the file read again.

use super::ledger::{self, Entry};
use sha2::{Digest, Sha256};
use std::io::{self, Read as _, Seek, SeekFrom};
use std::path::Path;

/// What a day's file is checked by: its first bytes, up to this many.
const HEAD: u64 = 4096;

/// How far a day's file has been read.
#[derive(Clone, Default)]
pub(super) struct Tail {
    /// The bytes read: through its last whole line.
    read: u64,
    /// The digest of its first bytes, up to `HEAD` of those read.
    head: [u8; 32],
}

/// What reading on found: each row with where its line begins.
pub(super) enum Read {
    /// Nothing written since.
    Same,
    /// The rows written since.
    Grew(Vec<(u64, Entry)>),
    /// Another file, or the first reading: every row of it.
    Whole(Vec<(u64, Entry)>),
    /// The file is not there.
    Gone,
}

/// A line that is no row.
struct Damaged;

impl Tail {
    pub fn read(&mut self, file: &Path) -> io::Result<Read> {
        match self.attempt(file)? {
            Ok(read) => Ok(read),
            Err(Damaged) => {
                ledger::read(file)?;
                *self = Self::default();
                match self.attempt(file)? {
                    Ok(read) => Ok(read),
                    Err(Damaged) => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} holds a line that is no row", file.display()),
                    )),
                }
            }
        }
    }

    fn attempt(&mut self, file: &Path) -> io::Result<Result<Read, Damaged>> {
        let mut opened = match fastctx_platform::disk::open(file) {
            Ok(opened) => opened,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                *self = Self::default();
                return Ok(Ok(Read::Gone));
            }
            Err(error) => return Err(error),
        };
        let len = opened.metadata()?.len();
        let same = self.read > 0 && self.read <= len && head(&mut opened, self.read)? == self.head;
        let from = if same { self.read } else { 0 };
        if from == len {
            if same {
                return Ok(Ok(Read::Same));
            }
            *self = Self::default();
            return Ok(Ok(Read::Whole(Vec::new())));
        }
        opened.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        (&mut opened).take(len - from).read_to_end(&mut bytes)?;
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let mut rows = Vec::new();
        let mut line = from;
        for raw in bytes[..end].split_inclusive(|b| *b == b'\n') {
            let content = &raw[..raw.len() - 1];
            if !super::quarantine::blank(content) {
                let Ok(entry) = serde_json::from_slice::<Entry>(content) else {
                    return Ok(Err(Damaged));
                };
                rows.push((line, entry));
            }
            line += raw.len() as u64;
        }
        self.read = from + end as u64;
        self.head = head(&mut opened, self.read)?;
        Ok(Ok(if same { Read::Grew(rows) } else { Read::Whole(rows) }))
    }
}

/// The digest of a file's first bytes, up to `HEAD` of the `read` given.
fn head(file: &mut std::fs::File, read: u64) -> io::Result<[u8; 32]> {
    let mut bytes = Vec::new();
    file.seek(SeekFrom::Start(0))?;
    file.take(read.min(HEAD)).read_to_end(&mut bytes)?;
    Ok(Sha256::digest(&bytes).into())
}
