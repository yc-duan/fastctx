//! A bounded read of one opened file, with observations before and after.

use std::io;
use std::path::Path;

use fastctx_platform::file_read::{self, Snapshot};
use fastctx_types::MachineName;
use fastctx_types::file_read::*;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use super::{Part, Sender};

pub async fn read(machine: MachineName, request: ReadFileRequest, sender: &Sender) -> FileReadResult {
    read_attachment(machine, request, None, None, None, sender).await
}

/// Reads `request` as `read` does, for a file the user sent: `expected`
/// is the file as it was found, and `version` names its bytes where the
/// file is made anew for each read (a picture decoded from a host's
/// record), whose own identity would differ every time.
pub async fn read_attachment(
    machine: MachineName,
    mut request: ReadFileRequest,
    attachment: Option<fastctx_types::attachments::AttachmentFile>,
    expected: Option<Snapshot>,
    version: Option<String>,
    sender: &Sender,
) -> FileReadResult {
    request.path = fastctx_platform::local_path(&request.path)
        .map_err(|e| error(FileReadErrorKind::InvalidInput, e.to_string(), 0))?
        .to_string_lossy()
        .into_owned();
    let path = request.path.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let path = Path::new(&path);
        if !path.is_absolute() {
            return Err(error(FileReadErrorKind::InvalidInput, "the file path must be absolute", 0));
        }
        let file = file_read::open(path).map_err(|e| io_error(e, 0))?;
        let metadata = file.metadata().map_err(|e| io_error(e, 0))?;
        if metadata.is_dir() {
            return Err(error(FileReadErrorKind::IsDirectory, "the path is a directory", 0));
        }
        if !metadata.is_file() {
            return Err(error(FileReadErrorKind::NotRegularFile, "the path is not an ordinary file", 0));
        }
        let snapshot = file_read::snapshot(&file, &metadata).map_err(|e| io_error(e, 0))?;
        let check = file.try_clone().map_err(|e| io_error(e, 0))?;
        Ok((file, check, snapshot))
    })
    .await
    .map_err(|e| error(FileReadErrorKind::Io, e.to_string(), 0))?;
    let (file, check, before) = opened?;
    if expected.is_some_and(|expected| expected != before) {
        return Err(error(FileReadErrorKind::Changed, "the verified attachment changed before reading", 0));
    }
    let initial = metadata(&before).map_err(|e| io_error(e, 0))?;
    let (offset, length) = match request.range {
        FileRange::All => (0, before.size),
        FileRange::Tail { bytes } => (before.size.saturating_sub(bytes), bytes.min(before.size)),
        FileRange::Range { offset, bytes } => {
            let offset = offset.min(before.size);
            (offset, bytes.min(before.size - offset))
        }
    };
    let version = Some(version.unwrap_or_else(|| identity_version(&before)));
    let info = FileReadInfo { machine, path: request.path.clone(), file: initial, offset, length, attachment, version };
    let mut file = tokio::fs::File::from_std(file);
    file.seek(io::SeekFrom::Start(offset)).await.map_err(|e| io_error(e, 0))?;
    sender.send(Ok(Part::Opened(info))).await.map_err(|e| error(FileReadErrorKind::Io, e.to_string(), 0))?;
    // One buffer is one existing data-message payload, not a read-size limit.
    let mut buffer = vec![0; fastctx_net::wire::MAX_DATA_MESSAGE];
    let mut bytes = 0;
    while bytes < length {
        let count = (length - bytes).min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..count]).await.map_err(|e| io_error(e, bytes))?;
        if read == 0 {
            return Err(error(FileReadErrorKind::Changed, "the file ended before the selected range was read", bytes));
        }
        sender
            .send(Ok(Part::Chunk { offset: offset + bytes, bytes: buffer[..read].to_vec() }))
            .await
            .map_err(|e| error(FileReadErrorKind::Io, e.to_string(), bytes))?;
        bytes += read as u64;
    }
    tokio::task::spawn_blocking(move || {
        let after = file_read::snapshot(&check, &check.metadata().map_err(|e| io_error(e, bytes))?)
            .map_err(|e| io_error(e, bytes))?;
        let observed = metadata(&after).map_err(|e| io_error(e, bytes))?;
        let current = match file_read::open(Path::new(&request.path)) {
            Ok(current) => current,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(FileReadError {
                    observed: Some(observed),
                    ..error(FileReadErrorKind::Changed, "the path disappeared while reading", bytes)
                });
            }
            Err(e) => return Err(io_error(e, bytes)),
        };
        let current = file_read::snapshot(&current, &current.metadata().map_err(|e| io_error(e, bytes))?)
            .map_err(|e| io_error(e, bytes))?;
        if before != after || before != current {
            return Err(FileReadError {
                observed: Some(metadata(&current).map_err(|e| io_error(e, bytes))?),
                ..error(FileReadErrorKind::Changed, "the file changed or was replaced while reading", bytes)
            });
        }
        Ok(FileReadComplete { bytes, file: observed })
    })
    .await
    .map_err(|e| error(FileReadErrorKind::Io, e.to_string(), bytes))?
}

/// The version of an opened file's bytes: its identity, size and change
/// times as the final check compares them, hashed, so the word says nothing
/// of the filesystem's numbers.
fn identity_version(snapshot: &Snapshot) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(snapshot.fingerprint().as_bytes());
    digest[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn metadata(snapshot: &Snapshot) -> io::Result<FileMetadata> {
    Ok(FileMetadata {
        size: snapshot.size,
        modified: jiff::Timestamp::try_from(snapshot.modified).map_err(io::Error::other)?,
    })
}

fn io_error(error: io::Error, bytes: u64) -> FileReadError {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => FileReadErrorKind::NotFound,
        io::ErrorKind::PermissionDenied => FileReadErrorKind::PermissionDenied,
        io::ErrorKind::IsADirectory => FileReadErrorKind::IsDirectory,
        _ => FileReadErrorKind::Io,
    };
    self::error(kind, error.to_string(), bytes)
}

fn error(kind: FileReadErrorKind, message: impl Into<String>, bytes: u64) -> FileReadError {
    FileReadError { kind, message: message.into(), bytes, observed: None }
}
