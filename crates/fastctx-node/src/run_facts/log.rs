//! JSON rows commit at their newline. Callers serialize writers and recovery reads.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Read, Seek, Write};
use std::path::Path;

/// Replays complete rows, removing only an unfinished final row. The caller must
/// hold the same record lock as the writer, including while applying each row.
pub(super) fn read(path: &Path, mut apply: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    recover(&mut file)?;
    file.rewind()?;
    let mut reader = io::BufReader::new(file);
    let mut row = Vec::new();
    let mut offset = 0;
    loop {
        row.clear();
        let length = reader.read_until(b'\n', &mut row)?;
        if length == 0 {
            return Ok(());
        }
        apply(&row)
            .map_err(|error| io::Error::new(error.kind(), format!("{} at byte {offset}: {error}", path.display())))?;
        offset += length as u64;
    }
}

pub(super) fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Publish the empty log before acknowledging any rows in a new file.
            fastctx_platform::publish_new(path, &[], fastctx_platform::Permissions::Private)?;
            OpenOptions::new().read(true).write(true).open(path)?
        }
        Err(error) => return Err(error),
    };
    let before = recover(&mut file)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        file.set_len(before)
            .and_then(|()| file.sync_all())
            .map_err(|rollback| io::Error::other(format!("{error}; restoring the previous log length: {rollback}")))?;
        return Err(error);
    }
    Ok(())
}

fn recover(file: &mut File) -> io::Result<u64> {
    let end = file.seek(io::SeekFrom::End(0))?;
    let mut before = end;
    let mut buffer = [0; 8192];
    while before != 0 {
        let length = before.min(buffer.len() as u64) as usize;
        before -= length as u64;
        file.seek(io::SeekFrom::Start(before))?;
        file.read_exact(&mut buffer[..length])?;
        if let Some(newline) = buffer[..length].iter().rposition(|byte| *byte == b'\n') {
            before += newline as u64 + 1;
            break;
        }
    }
    if before != end {
        // Byte framing also recovers a row cut in the middle of a UTF-8 character.
        // Flush the shorter length before a new append can reuse the torn tail.
        file.set_len(before)?;
        file.sync_all()?;
    }
    file.seek(io::SeekFrom::Start(before))?;
    Ok(before)
}
