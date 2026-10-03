//! Directories only their owner can enter.
//!
//! The World keeps device keys, residents' records and every resident's
//! output under one directory per user; nobody else on the machine may read
//! it. On Unix that is mode 0700. On Windows it is a protected access list
//! granting full control to the current user's SID and to SYSTEM; the user
//! is named by SID rather than as the directory's owner, because an elevated
//! process makes Administrators the owner, after which the same user without
//! elevation could no longer use the directory.

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
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SetFileSecurityW, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub fn make_private(path: &Path) -> io::Result<()> {
        let sid = current_user_sid()?;
        let sddl: Vec<u16> =
            OsStr::new(&format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)")).encode_wide().chain([0]).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: a NUL-terminated SDDL string; the descriptor is freed below.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut())
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated path and a valid descriptor.
        let applied = unsafe {
            SetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor)
        };
        let error = io::Error::last_os_error();
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(descriptor) };
        if applied == 0 {
            return Err(error);
        }
        Ok(())
    }

    /// The current process token's user SID, as text such as `S-1-5-21-…`.
    fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: querying this process's own token.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut needed = 0u32;
            // SAFETY: a size query with no buffer.
            unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
            // SAFETY: the buffer is at least `needed` bytes and aligned.
            if unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: GetTokenInformation filled a TOKEN_USER.
            let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
            let mut text: *mut u16 = ptr::null_mut();
            // SAFETY: a valid SID; the string is freed below.
            if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string.
            let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
            // SAFETY: `length` units precede the NUL.
            let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
            // SAFETY: allocated by ConvertSidToStringSidW.
            unsafe { LocalFree(text.cast()) };
            Ok(sid)
        })();
        // SAFETY: the token handle is owned here.
        unsafe { CloseHandle(token) };
        result
    }
}
