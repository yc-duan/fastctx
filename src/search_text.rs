//! Immutable UTF-8 search input shared by ripgrep callbacks and captured ranges.

use crate::file_snapshot::{SnapshotByteRange, SnapshotReader};
use crate::operation::{WorkCheckpoint, WorkStop};
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::sync::Arc;

const MEMORY_TEXT_LIMIT: usize = 8 * 1024 * 1024;
const TEXT_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct TempText {
    file: tempfile::NamedTempFile,
}

impl TempText {
    fn open_reader(&self) -> io::Result<TextReader<'static>> {
        Ok(TextReader::Temp(BufReader::new(self.file.reopen()?)))
    }

    fn read_range(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        let range_len = range.end.checked_sub(range.start).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "captured text range is reversed",
            )
        })?;
        let length = usize::try_from(range_len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "captured text range does not fit in memory",
            )
        })?;
        let mut file = self.file.reopen()?;
        file.seek(SeekFrom::Start(range.start))?;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

#[derive(Debug)]
enum TextStorage {
    Snapshot(SnapshotByteRange),
    Memory(Arc<[u8]>),
    Temp(Arc<TempText>),
}

/// One immutable decoded UTF-8 source whose offsets are ripgrep's absolute offsets.
#[derive(Debug)]
pub(crate) struct SearchText {
    len: u64,
    storage: TextStorage,
}

impl SearchText {
    /// Reuses a validated UTF-8 snapshot suffix without copying its memory or temp backing.
    pub(crate) fn from_snapshot(range: SnapshotByteRange) -> Arc<Self> {
        Arc::new(Self {
            len: range.len(),
            storage: TextStorage::Snapshot(range),
        })
    }

    /// Captures a validated UTF-8 reader once, spilling above 8 MiB without an unbounded heap copy.
    pub(crate) fn capture(
        mut reader: impl Read,
        operation: Option<&dyn WorkCheckpoint>,
    ) -> Result<Arc<Self>, SearchTextFailure> {
        let mut memory = Vec::with_capacity(MEMORY_TEXT_LIMIT.min(TEXT_CHUNK_BYTES));
        let mut temp: Option<tempfile::NamedTempFile> = None;
        let mut total = 0_u64;
        let mut chunk = [0_u8; TEXT_CHUNK_BYTES];

        loop {
            check_operation(operation)?;
            let read = reader.read(&mut chunk).map_err(SearchTextFailure::Io)?;
            check_operation(operation)?;
            if read == 0 {
                break;
            }
            total = total.checked_add(read as u64).ok_or_else(|| {
                SearchTextFailure::Io(io::Error::other("search text is too large"))
            })?;

            if temp.is_none() && memory.len().saturating_add(read) > MEMORY_TEXT_LIMIT {
                let mut promoted = tempfile::NamedTempFile::new().map_err(SearchTextFailure::Io)?;
                promoted.write_all(&memory).map_err(SearchTextFailure::Io)?;
                memory.clear();
                temp = Some(promoted);
            }
            if let Some(file) = &mut temp {
                file.write_all(&chunk[..read])
                    .map_err(SearchTextFailure::Io)?;
            } else {
                memory.extend_from_slice(&chunk[..read]);
            }
        }

        check_operation(operation)?;
        let storage = if let Some(mut file) = temp {
            file.flush().map_err(SearchTextFailure::Io)?;
            check_operation(operation)?;
            TextStorage::Temp(Arc::new(TempText { file }))
        } else {
            TextStorage::Memory(Arc::from(memory))
        };
        Ok(Arc::new(Self {
            len: total,
            storage,
        }))
    }

    pub(crate) fn memory_bytes(&self) -> Option<&[u8]> {
        match &self.storage {
            TextStorage::Snapshot(range) => range.memory_bytes(),
            TextStorage::Memory(bytes) => Some(bytes),
            TextStorage::Temp(_) => None,
        }
    }

    pub(crate) fn open_reader(&self) -> io::Result<TextReader<'_>> {
        match &self.storage {
            TextStorage::Snapshot(range) => Ok(TextReader::Snapshot(range.open_reader()?)),
            TextStorage::Memory(bytes) => Ok(TextReader::Memory(Cursor::new(bytes))),
            TextStorage::Temp(temp) => temp.open_reader(),
        }
    }

    pub(crate) fn range_str(&self, range: Range<u64>) -> io::Result<RangeText<'_>> {
        self.validate_range(&range)?;
        match &self.storage {
            TextStorage::Snapshot(backing) => {
                if let Some(bytes) = backing.memory_bytes() {
                    let text = borrowed_utf8_range(bytes, range)?;
                    Ok(RangeText::Borrowed(text))
                } else {
                    let bytes = backing.read_range(range)?;
                    let text = String::from_utf8(bytes)
                        .map_err(|error| invalid_utf8_range(error.utf8_error()))?;
                    Ok(RangeText::Owned(Arc::from(text)))
                }
            }
            TextStorage::Memory(bytes) => {
                let text = borrowed_utf8_range(bytes, range)?;
                Ok(RangeText::Borrowed(text))
            }
            TextStorage::Temp(temp) => {
                let bytes = temp.read_range(range)?;
                let text = String::from_utf8(bytes)
                    .map_err(|error| invalid_utf8_range(error.utf8_error()))?;
                Ok(RangeText::Owned(Arc::from(text)))
            }
        }
    }

    fn validate_range(&self, range: &Range<u64>) -> io::Result<()> {
        if range.start <= range.end && range.end <= self.len {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ripgrep reported a byte range outside its immutable search input",
            ))
        }
    }

    #[cfg(test)]
    pub(crate) fn is_temp(&self) -> bool {
        self.memory_bytes().is_none()
    }
}

fn borrowed_utf8_range(bytes: &[u8], range: Range<u64>) -> io::Result<&str> {
    let start = usize::try_from(range.start).map_err(|_| text_range_error())?;
    let end = usize::try_from(range.end).map_err(|_| text_range_error())?;
    let bytes = bytes.get(start..end).ok_or_else(text_range_error)?;
    std::str::from_utf8(bytes).map_err(invalid_utf8_range)
}

fn text_range_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "captured text range lies outside its immutable search input",
    )
}

fn invalid_utf8_range(error: std::str::Utf8Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("ripgrep reported a range that is not valid UTF-8: {error}"),
    )
}

fn check_operation(operation: Option<&dyn WorkCheckpoint>) -> Result<(), SearchTextFailure> {
    match operation.map(WorkCheckpoint::check_work) {
        Some(Err(stop)) => Err(SearchTextFailure::Stopped(stop)),
        Some(Ok(())) | None => Ok(()),
    }
}

/// A decoded text range, borrowed in memory and owned only when read from temp storage.
#[derive(Debug)]
pub(crate) enum RangeText<'a> {
    Borrowed(&'a str),
    Owned(Arc<str>),
}

impl RangeText<'_> {
    #[cfg(test)]
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(text) => text,
            Self::Owned(text) => text,
        }
    }
}

/// A reader over the same immutable decoded UTF-8 backing.
pub(crate) enum TextReader<'a> {
    Snapshot(SnapshotReader<'a>),
    Memory(Cursor<&'a [u8]>),
    Temp(BufReader<File>),
}

impl Read for TextReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Snapshot(reader) => reader.read(output),
            Self::Memory(reader) => reader.read(output),
            Self::Temp(reader) => reader.read(output),
        }
    }
}

#[derive(Debug)]
pub(crate) enum SearchTextFailure {
    Io(io::Error),
    Stopped(WorkStop),
}

#[cfg(test)]
mod tests {
    use super::{MEMORY_TEXT_LIMIT, SearchText, TEXT_CHUNK_BYTES, TextStorage};
    use std::io::{self, Cursor};

    #[test]
    fn memory_and_temp_ranges_share_the_exact_search_offsets() {
        let memory = SearchText::capture(Cursor::new("前hit后".as_bytes()), None).unwrap();
        assert!(!memory.is_temp());
        assert_eq!(memory.range_str(3..6).unwrap().as_str(), "hit");

        let mut large = vec![b'x'; MEMORY_TEXT_LIMIT + 17];
        large[MEMORY_TEXT_LIMIT + 3..MEMORY_TEXT_LIMIT + 6].copy_from_slice(b"hit");
        let temp = SearchText::capture(Cursor::new(large), None).unwrap();
        assert!(temp.is_temp());
        assert_eq!(
            temp.range_str((MEMORY_TEXT_LIMIT + 3) as u64..(MEMORY_TEXT_LIMIT + 6) as u64)
                .unwrap()
                .as_str(),
            "hit"
        );
    }

    #[test]
    fn temp_ranges_preserve_multibyte_text_across_capture_chunks() {
        let match_start = MEMORY_TEXT_LIMIT + TEXT_CHUNK_BYTES - 1;
        let mut large = vec![b'x'; match_start];
        large.extend_from_slice("界hit".as_bytes());
        let match_end = large.len();
        let temp = SearchText::capture(Cursor::new(large), None).unwrap();

        assert!(temp.is_temp());
        assert_eq!(
            temp.range_str(match_start as u64..match_end as u64)
                .unwrap()
                .as_str(),
            "界hit"
        );
    }

    #[test]
    fn temp_range_io_failures_are_returned_instead_of_hidden() {
        let temp =
            SearchText::capture(Cursor::new(vec![b'x'; MEMORY_TEXT_LIMIT + 1]), None).unwrap();
        let TextStorage::Temp(storage) = &temp.storage else {
            panic!("expected temp-backed text");
        };
        storage.file.as_file().set_len(0).unwrap();

        let error = temp.range_str(0..1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
