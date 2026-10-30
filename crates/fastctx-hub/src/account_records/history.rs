//! Preserve revisions and accepted submission receipts before overwriting the
//! current plan. Replaying a submission returns its own result, not a new edit.

use fastctx_types::{account_records::Record, plan::Body};
use std::{io, path::Path};

fn location(directory: &Path) -> io::Result<std::path::PathBuf> {
    // Canonical parents carry the Windows extended-length prefix. Submission
    // paths are deeper than ordinary records and must not inherit MAX_PATH.
    Ok(std::fs::canonicalize(directory.parent().expect("account directory"))?
        .join(directory.file_name().expect("history directory")))
}

fn immutable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fastctx_platform::ensure_private_dir(path.parent().expect("history directory"))?;
    match std::fs::read(path) {
        Ok(old) if old == bytes => Ok(()),
        Ok(_) => Err(io::Error::other("an immutable plan revision or submission was reused")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fastctx_platform::publish(path, bytes, fastctx_platform::Permissions::Private)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn preserve(current: &Path, directory: &Path) -> io::Result<()> {
    let bytes = match std::fs::read(current) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let directory = location(directory)?;
    let record: Record = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    immutable(&directory.join(format!("{}.json", record.metadata.version)), &bytes)?;
    if !record.metadata.deleted {
        let body: Body = serde_json::from_value(record.body).map_err(io::Error::other)?;
        if let Some(id) = body.submission {
            if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(io::Error::other("invalid stored plan submission identity"));
            }
            immutable(&directory.join("submissions").join(format!("{id}.json")), &bytes)?;
        }
    }
    Ok(())
}

pub(crate) fn submitted(current: Option<&Record>, directory: &Path, id: &str) -> Result<Option<Record>, String> {
    if let Some(current) = current.filter(|record| !record.metadata.deleted) {
        let body: Body = serde_json::from_value(current.body.clone()).map_err(|error| error.to_string())?;
        if body.submission.as_deref().map(String::as_str) == Some(id) {
            return Ok(Some(current.clone()));
        }
    }
    let path = location(directory).map_err(|error| error.to_string())?.join("submissions").join(format!("{id}.json"));
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// A tombstone is committed first; startup retries removal after a crash.
pub(crate) fn delete(directory: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(location(directory)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn receipt(directory: &Path, id: &str, record: &Record) -> io::Result<()> {
    immutable(
        &location(directory)?.join("submissions").join(format!("{id}.json")),
        &serde_json::to_vec(record).map_err(io::Error::other)?,
    )
}
