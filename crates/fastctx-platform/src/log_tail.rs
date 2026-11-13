//! Reading a rolling log as it grows (`rolling_log`): the file it is now and
//! the one it rolled into (`<name>.log.1`), each named by a digest of its
//! first line, read on from where a reader stands a piece at a time, whole
//! lines only. A file rolled by renaming keeps its name, so a reader finishes
//! it before going on to the new one; one cut down by copying has another
//! name, and the reader goes on from the start of the file the log is now.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

/// The most of a file's start its name is looked for in.
const FIRST_LINE: usize = 4096;

/// Where a reader of a rolling log stands: the file, by its name, and how
/// far into it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tail {
    pub file: String,
    pub offset: u64,
}

/// A piece of a log: the file it is of, where it begins, and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub file: String,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

/// The name of an open log file: a digest of its first line, none while it
/// has no whole line.
fn name(file: &mut File) -> io::Result<Option<String>> {
    file.seek(SeekFrom::Start(0))?;
    let mut start = Vec::with_capacity(FIRST_LINE);
    file.by_ref().take(FIRST_LINE as u64).read_to_end(&mut start)?;
    let Some(end) = start.iter().position(|&byte| byte == b'\n') else {
        // A first line longer than is looked at is named by its start.
        return Ok((start.len() == FIRST_LINE).then(|| digest(&start)));
    };
    Ok(Some(digest(&start[..end])))
}

/// FNV-1a over `bytes`, as sixteen hex digits.
fn digest(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn open(path: &Path) -> io::Result<Option<File>> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The whole lines of `file` from `offset`, at most `most` bytes; a single
/// line longer than that is given cut. None when no whole line is there.
fn lines(file: &mut File, offset: u64, most: usize) -> io::Result<Option<Vec<u8>>> {
    let length = file.metadata()?.len();
    if offset >= length {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(most.min((length - offset) as usize));
    file.by_ref().take(most as u64).read_to_end(&mut bytes)?;
    match bytes.iter().rposition(|&byte| byte == b'\n') {
        Some(end) => bytes.truncate(end + 1),
        None if bytes.len() == most => {}
        None => return Ok(None),
    }
    Ok(Some(bytes))
}

/// What comes next of the log at `path` for a reader standing at `at` (a
/// default one stands before everything kept), at most `most` bytes of
/// whole lines: None when nothing whole is new. The piece says where it
/// begins; the reader then stands at its end.
pub fn next(path: &Path, at: &Tail, most: usize) -> io::Result<Option<Piece>> {
    let rolled = path.with_extension("log.1");
    let mut now = open(path)?;
    let now_name = match now.as_mut() {
        Some(file) => name(file)?,
        None => None,
    };
    // Still on the file the log is now.
    if !at.file.is_empty() && now_name.as_deref() == Some(at.file.as_str()) {
        let file = now.as_mut().expect("named");
        return Ok(lines(file, at.offset, most)?.map(|bytes| Piece { file: at.file.clone(), offset: at.offset, bytes }));
    }
    // The file it read rolled: finish it first; from the start, the rolled
    // file comes before the one the log is now.
    if let Some(mut old) = open(&rolled)?
        && let Some(old_name) = name(&mut old)?
        && (at.file.is_empty() || old_name == at.file)
        && Some(&old_name) != now_name.as_ref()
    {
        let offset = if at.file.is_empty() { 0 } else { at.offset };
        if let Some(bytes) = lines(&mut old, offset, most)? {
            return Ok(Some(Piece { file: old_name, offset, bytes }));
        }
    }
    let (Some(file), Some(file_name)) = (now.as_mut(), now_name) else { return Ok(None) };
    Ok(lines(file, 0, most)?.map(|bytes| Piece { file: file_name, offset: 0, bytes }))
}

/// The time the first line of `bytes` begins with, in milliseconds since
/// 1970, as the logs write it; none when it does not begin with one.
pub fn first_time(bytes: &[u8]) -> Option<i64> {
    let end = bytes.iter().take(64).position(|&byte| byte == b' ')?;
    let text = std::str::from_utf8(&bytes[..end]).ok()?;
    text.parse::<jiff::Timestamp>().ok().map(|at| at.as_millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_a_log_through_its_roll() {
        let dir = std::env::temp_dir().join(format!("fastctx-log-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("node.log");
        std::fs::write(&path, "2026-11-13T00:00:00Z one\n2026-11-13T00:00:01Z two\npart").unwrap();
        let first = next(&path, &Tail::default(), 1 << 20).unwrap().unwrap();
        assert_eq!(first.offset, 0);
        assert_eq!(first.bytes, b"2026-11-13T00:00:00Z one\n2026-11-13T00:00:01Z two\n");
        assert_eq!(first_time(&first.bytes), Some(1_794_528_000_000));
        let at = Tail { file: first.file.clone(), offset: first.bytes.len() as u64 };
        assert_eq!(next(&path, &at, 1 << 20).unwrap(), None);
        // The line completes, then the log rolls by renaming.
        std::fs::write(&path, "2026-11-13T00:00:00Z one\n2026-11-13T00:00:01Z two\npartial\n").unwrap();
        std::fs::rename(&path, dir.join("node.log.1")).unwrap();
        std::fs::write(&path, "2026-11-13T00:01:00Z three\n").unwrap();
        let rest = next(&path, &at, 1 << 20).unwrap().unwrap();
        assert_eq!((rest.file.as_str(), rest.bytes.as_slice()), (first.file.as_str(), b"partial\n".as_slice()));
        let at = Tail { file: rest.file.clone(), offset: rest.offset + rest.bytes.len() as u64 };
        let new = next(&path, &at, 1 << 20).unwrap().unwrap();
        assert_ne!(new.file, first.file);
        assert_eq!((new.offset, new.bytes.as_slice()), (0, b"2026-11-13T00:01:00Z three\n".as_slice()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
