//! Linux mount identities and paths, including bind mounts and non-UTF-8 names.

use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

/// One mount visible in this process's mount namespace.
#[derive(Debug)]
pub struct Mount {
    pub id: u64,
    pub device: String,
    pub root: PathBuf,
    pub point: PathBuf,
    pub kind: String,
}

/// Reads the complete table. An unreadable or malformed table is an error,
/// never an empty set of mounts.
pub fn read() -> io::Result<Vec<Mount>> {
    let bytes = std::fs::read("/proc/self/mountinfo")
        .map_err(|error| io::Error::new(error.kind(), format!("reading /proc/self/mountinfo: {error}")))?;
    let mut mounts = Vec::new();
    for line in bytes.split(|&byte| byte == b'\n').filter(|line| !line.is_empty()) {
        let fields: Vec<_> = line.split(|&byte| byte == b' ').collect();
        let separator = fields.iter().position(|field| *field == b"-").ok_or_else(invalid)?;
        if separator < 6 || fields.len() < separator + 4 {
            return Err(invalid());
        }
        let text = |field: &[u8]| std::str::from_utf8(field).map(str::to_owned).map_err(|_| invalid());
        let id = text(fields[0])?.parse().map_err(|_| invalid())?;
        mounts.push(Mount {
            id,
            device: text(fields[2])?,
            root: path(fields[3])?,
            point: path(fields[4])?,
            kind: text(fields[separator + 1])?,
        });
    }
    if mounts.is_empty() {
        return Err(invalid());
    }
    Ok(mounts)
}

/// Identifies the mount containing an absolute path without following its
/// final symlink. Mount IDs use the same namespace as STATX_MNT_ID.
pub fn id(path: &Path) -> io::Result<u64> {
    if !path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "mount lookup requires an absolute path"));
    }
    read()?
        .into_iter()
        .filter(|mount| path.starts_with(&mount.point))
        .max_by_key(|mount| mount.point.components().count())
        .map(|mount| mount.id)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, format!("no mountinfo entry covers {}", path.display()))
        })
}

fn path(field: &[u8]) -> io::Result<PathBuf> {
    let mut bytes = Vec::with_capacity(field.len());
    let mut at = 0;
    while at < field.len() {
        if field[at] == b'\\' {
            let digits = field.get(at + 1..at + 4).ok_or_else(invalid)?;
            if !digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                return Err(invalid());
            }
            let value =
                u16::from(digits[0] - b'0') * 64 + u16::from(digits[1] - b'0') * 8 + u16::from(digits[2] - b'0');
            bytes.push(u8::try_from(value).map_err(|_| invalid())?);
            at += 4;
        } else {
            bytes.push(field[at]);
            at += 1;
        }
    }
    let path = PathBuf::from(OsString::from_vec(bytes));
    if !path.is_absolute() {
        return Err(invalid());
    }
    Ok(path)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid /proc/self/mountinfo; mount boundaries are unknown")
}
