//! Unix account records, with a checked environment fallback for the caller.

use std::ffi::{CStr, OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

/// Owned fields of an account record; no pointers into the lookup buffer escape.
pub struct Account {
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
    pub name: OsString,
    pub home: OsString,
    pub shell: OsString,
}

/// Looks up exactly `uid`. An absent record is distinct from a lookup failure.
pub fn lookup(uid: libc::uid_t) -> io::Result<Option<Account>> {
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    loop {
        // SAFETY: getpwuid_r writes into these initialized buffers; strings
        // are copied before the buffer is resized or dropped.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found = std::ptr::null_mut();
        let code = unsafe { libc::getpwuid_r(uid, &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) };
        if code == libc::ERANGE {
            let length = buffer.len().checked_mul(2).ok_or_else(|| io::Error::from_raw_os_error(libc::ENOMEM))?;
            buffer.resize(length, 0);
            continue;
        }
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        if found.is_null() {
            return Ok(None);
        }
        let text = |field: *const libc::c_char| -> OsString {
            if field.is_null() {
                return OsString::new();
            }
            // SAFETY: successful getpwuid_r supplies terminated strings in buffer.
            OsStr::from_bytes(unsafe { CStr::from_ptr(field) }.to_bytes()).into()
        };
        let shell = text(entry.pw_shell);
        return Ok(Some(Account {
            uid: entry.pw_uid,
            gid: entry.pw_gid,
            name: text(entry.pw_name),
            home: text(entry.pw_dir),
            shell: if shell.is_empty() { "/bin/sh".into() } else { shell },
        }));
    }
}

/// Reads the caller's account. Static libc may not see an NSS-only user;
/// only that caller may substitute HOME and USER, and HOME must be its directory.
pub fn current() -> io::Result<Account> {
    // SAFETY: getuid and getgid have no preconditions.
    let uid = unsafe { libc::getuid() };
    if let Some(account) = lookup(uid)? {
        return Ok(account);
    }
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| io::Error::other(format!("user {uid} has no account record and HOME is unset")))?;
    let path = Path::new(&home);
    if !path.is_absolute() {
        return Err(io::Error::other("HOME must be absolute when the account record is unavailable"));
    }
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("HOME must be a directory owned by user {uid}"),
        ));
    }
    let name = std::env::var_os("USER")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| io::Error::other(format!("user {uid} has no account record and USER is unset")))?;
    Ok(Account { uid, gid: unsafe { libc::getgid() }, name, home, shell: "/bin/sh".into() })
}
