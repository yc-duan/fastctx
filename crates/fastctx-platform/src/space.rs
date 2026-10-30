//! Available storage on the filesystem that will hold a path. Unlike disk
//! enumeration, a failed measurement is an error rather than an omitted disk.

use std::{io, path::Path};

pub fn available(path: &Path) -> io::Result<u64> {
    let parent = path
        .ancestors()
        .find(|path| path.is_dir())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no existing ancestor for storage measurement"))?;
    let path = std::fs::canonicalize(parent)?;
    measure(&path)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast, reason = "libc filesystem counters differ in width across Unix targets")]
fn measure(path: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is terminated and status points to writable storage.
    if unsafe { libc::statvfs(path.as_ptr(), status.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs initialized the structure on success.
    let status = unsafe { status.assume_init() };
    Ok((status.f_bavail as u64).saturating_mul(status.f_frsize as u64))
}

#[cfg(windows)]
fn measure(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt as _;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0;
    // SAFETY: path is terminated; free is writable and optional outputs are null.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(free)
}
