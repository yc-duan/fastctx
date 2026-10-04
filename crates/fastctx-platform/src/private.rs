//! Directories only their owner can enter.
//!
//! The World keeps device keys, residents' records and every resident's
//! output under one directory per user; nobody else on the machine may read
//! it. On Unix that is mode 0700. On Windows it is a protected access list
//! admitting only the current user and SYSTEM.

use std::io;
use std::path::Path;

/// Creates `path` (and missing parents) and makes `path` itself private.
/// Its contents inherit the protection.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    imp::make_private(path)
}

#[cfg(unix)]
mod imp {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    pub fn make_private(path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() {
            return Err(io::Error::other(format!("{} is not a directory", path.display())));
        }
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };

    use crate::windows_user::OwnerOnly;

    pub fn make_private(path: &Path) -> io::Result<()> {
        let access = OwnerOnly::new(true)?;
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated path and a valid descriptor.
        let applied = unsafe {
            SetFileSecurityW(
                name.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                access.descriptor(),
            )
        };
        if applied == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
