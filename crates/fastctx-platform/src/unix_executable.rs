//! Executability for the effective identity, including old syscall sandboxes.

use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

/// Uses the kernel's ACL-aware check first. Only ENOSYS or EPERM permits a
/// mode-bit approximation; it cannot reproduce ACL or noexec mount policy.
pub fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else { return false };
    if !metadata.is_file() {
        return false;
    }
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: a terminated path; faccessat does not retain the pointer.
    if unsafe { libc::faccessat(libc::AT_FDCWD, name.as_ptr(), libc::X_OK, libc::AT_EACCESS) } == 0 {
        return true;
    }
    if !matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) {
        return false;
    }
    // SAFETY: identity queries have no preconditions.
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    let mode = metadata.mode();
    if uid == 0 {
        return mode & 0o111 != 0;
    }
    if metadata.uid() == uid {
        return mode & 0o100 != 0;
    }
    let same_group = gid == metadata.gid() || {
        // SAFETY: the first query asks only for the required array size.
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        if count < 0 {
            return false;
        }
        let mut groups = vec![0 as libc::gid_t; count as usize];
        // SAFETY: groups has the requested capacity. A concurrent group change
        // fails the check rather than granting access using incomplete groups.
        let read = unsafe { libc::getgroups(count, groups.as_mut_ptr()) };
        if read < 0 {
            return false;
        }
        groups[..read as usize].contains(&metadata.gid())
    };
    mode & if same_group { 0o010 } else { 0o001 } != 0
}
