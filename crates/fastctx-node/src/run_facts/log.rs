//! JSON rows commit at their newline. Callers serialize writers and recovery reads.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::Path;

pub(super) fn append(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = match fastctx_platform::disk::open_with(OpenOptions::new().read(true).write(true), path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Publish the empty log before acknowledging any rows in a new file.
            fastctx_platform::publish_new(path, &[], fastctx_platform::Permissions::Private)?;
            fastctx_platform::disk::open_with(OpenOptions::new().read(true).write(true), path)?
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

pub(super) fn repair(path: &Path) -> io::Result<()> {
    match fastctx_platform::disk::open_with(OpenOptions::new().read(true).write(true), path) {
        Ok(mut file) => recover(&mut file).map(|_| ()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
