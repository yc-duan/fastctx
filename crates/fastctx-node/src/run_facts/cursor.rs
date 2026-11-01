//! Bounded reads of executor journals. A cursor advances only over complete rows.

use std::io::{self, BufRead, Seek};
use std::path::Path;

const ROWS: usize = 128;
const BYTES: usize = 1024 * 1024;

pub(super) fn read<T>(
    path: &Path,
    offset: &mut u64,
    mut decode: impl FnMut(&[u8]) -> io::Result<T>,
) -> io::Result<Vec<T>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() < *offset {
        return Err(io::Error::other("the executor journal became shorter than its acknowledged cursor"));
    }
    let mut reader = io::BufReader::new(file);
    reader.seek(io::SeekFrom::Start(*offset))?;
    let mut result = Vec::new();
    let mut bytes = 0;
    let mut row = Vec::new();
    while result.len() < ROWS && bytes < BYTES {
        row.clear();
        let count = reader.read_until(b'\n', &mut row)?;
        if count == 0 || row.last() != Some(&b'\n') {
            break;
        }
        result.push(decode(&row)?);
        bytes += count;
        *offset += count as u64;
    }
    Ok(result)
}

pub(super) fn json<T: serde::de::DeserializeOwned>(path: &Path, offset: &mut u64) -> io::Result<Vec<T>> {
    read(path, offset, |row| serde_json::from_slice(row).map_err(io::Error::other))
}
