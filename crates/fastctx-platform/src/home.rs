//! Where fastctx keeps its things for this user on this machine.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

/// The directory holding everything fastctx keeps for this user:
/// `FASTCTX_HOME` when it is set, which lets a test or a second World run
/// beside the real one, and `.fastctx` in the user's home otherwise.
pub fn fastctx_root() -> io::Result<PathBuf> {
    match std::env::var_os("FASTCTX_HOME") {
        Some(root) => absolute("FASTCTX_HOME", root),
        None => Ok(user_home()?.join(".fastctx")),
    }
}

/// The user's home directory. On Unix it is `HOME`, as every program there
/// takes it, and the account database when `HOME` is unset. On Windows it
/// is the profile directory as the system records it, never a variable
/// that a shell may have respelled.
pub fn user_home() -> io::Result<PathBuf> {
    imp::user_home()
}

fn absolute(what: &str, path: OsString) -> io::Result<PathBuf> {
    let path = crate::local_path(PathBuf::from(path))?;
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} must be an absolute path, not {}", path.display()),
        ))
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, OsStr};
    use std::io;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::PathBuf;

    pub fn user_home() -> io::Result<PathBuf> {
        if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
            return super::absolute("HOME", home);
        }
        // SAFETY: getpwuid_r writes into the buffers given; the entry's
        // strings point into `buffer`, which outlives their use.
        unsafe {
            let mut entry: libc::passwd = std::mem::zeroed();
            let mut found: *mut libc::passwd = std::ptr::null_mut();
            let mut buffer = vec![0 as libc::c_char; 16 * 1024];
            let code = libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found);
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code));
            }
            if found.is_null() || entry.pw_dir.is_null() {
                return Err(io::Error::other("HOME is unset and this user has no entry in the account database"));
            }
            super::absolute("the account's home", OsStr::from_bytes(CStr::from_ptr(entry.pw_dir).to_bytes()).into())
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt as _;
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;

    use crate::windows_user::with_current_token;

    pub fn user_home() -> io::Result<PathBuf> {
        let profile = with_current_token(|token| {
            let mut length = 0u32;
            // SAFETY: a size query with no buffer.
            let sized = unsafe { GetUserProfileDirectoryW(token, std::ptr::null_mut(), &mut length) };
            let error = io::Error::last_os_error();
            if sized != 0 || error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
                return Err(error);
            }
            let mut buffer = vec![0u16; length as usize];
            // SAFETY: the buffer holds `length` units, as the system asked.
            if unsafe { GetUserProfileDirectoryW(token, buffer.as_mut_ptr(), &mut length) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
            Ok(OsString::from_wide(&buffer[..end]))
        })?;
        super::absolute("the profile directory", profile)
    }
}
