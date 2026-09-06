//! Service identity derives from the OS user and explicit FastCtx HOME, not inherited HOME.

use crate::control::paths::ControlPaths;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(super) fn scope(paths: &ControlPaths, user: Option<&str>) -> Result<String, String> {
    let canonical = dunce::canonicalize(&paths.home)
        .map_err(|error| format!("Cannot resolve the node HOME: {error}"))?;
    let mut hash = Sha256::new();
    hash.update(crate::session::native_bytes(canonical.as_os_str()));
    #[cfg(windows)]
    hash.update(windows_sid(user)?.as_bytes());
    #[cfg(not(windows))]
    {
        let _ = user;
        hash.update(unix_uid().to_le_bytes());
    }
    Ok(hex::encode(hash.finalize())[..12].to_string())
}

pub(super) fn is_default_home(paths: &ControlPaths) -> Result<bool, String> {
    let actual = dunce::canonicalize(os_home()?)
        .map_err(|error| format!("Cannot resolve the OS user profile: {error}"))?;
    let requested = dunce::canonicalize(&paths.home)
        .map_err(|error| format!("Cannot resolve the node HOME: {error}"))?;
    Ok(actual == requested)
}

#[cfg(unix)]
pub(super) fn unix_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(unix)]
pub(super) fn os_home() -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = vec![0u8; 16384];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found = std::ptr::null_mut();
        // SAFETY: record, result pointer and scratch buffer remain valid for the call.
        let code = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if code == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if code != 0 || found.is_null() {
            return Err(format!(
                "Cannot resolve the OS user's home directory (getpwuid_r: {code})."
            ));
        }
        // SAFETY: getpwuid_r initialized record and its pointers refer to the live buffer.
        let home = unsafe { std::ffi::CStr::from_ptr(record.assume_init().pw_dir) };
        return Ok(PathBuf::from(std::ffi::OsStr::from_bytes(home.to_bytes())));
    }
}

#[cfg(windows)]
fn token() -> Result<std::os::windows::io::OwnedHandle, String> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut handle = std::ptr::null_mut();
    // SAFETY: the output points to a valid handle slot; the returned token is owned here.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
        return Err(format!(
            "Cannot inspect the Windows service user: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle) })
}

#[cfg(windows)]
pub(super) fn os_home() -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;
    let token = token()?;
    let mut count = 0;
    // SAFETY: the first call sizes the UTF-16 buffer, the second fills owned storage.
    unsafe { GetUserProfileDirectoryW(token.as_raw_handle(), std::ptr::null_mut(), &mut count) };
    if count == 0 {
        return Err(format!(
            "Cannot size the Windows user profile: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut buffer = vec![0u16; count as usize];
    if unsafe { GetUserProfileDirectoryW(token.as_raw_handle(), buffer.as_mut_ptr(), &mut count) }
        == 0
    {
        return Err(format!(
            "Cannot read the Windows user profile: {}",
            std::io::Error::last_os_error()
        ));
    }
    let length = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length],
    )))
}

#[cfg(windows)]
pub(super) fn windows_sid(user: Option<&str>) -> Result<String, String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, LookupAccountNameW, TOKEN_USER, TokenUser,
    };
    let mut bytes = 0;
    let (buffer, sid) = if let Some(user) = user {
        let name = user.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let mut domain_length = 0;
        let mut usage = 0;
        // SAFETY: sizing outputs are valid; the second call uses aligned SID storage.
        unsafe {
            LookupAccountNameW(
                std::ptr::null(),
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut bytes,
                std::ptr::null_mut(),
                &mut domain_length,
                &mut usage,
            )
        };
        if bytes == 0 {
            return Err(format!(
                "Cannot resolve the service account: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        let mut domain = vec![0u16; domain_length as usize];
        if unsafe {
            LookupAccountNameW(
                std::ptr::null(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                domain.as_mut_ptr(),
                &mut domain_length,
                &mut usage,
            )
        } == 0
        {
            return Err(format!(
                "Cannot resolve the service account: {}",
                std::io::Error::last_os_error()
            ));
        }
        let sid = buffer.as_mut_ptr().cast();
        (buffer, sid)
    } else {
        let token = token()?;
        // SAFETY: TOKEN_USER and its SID stay inside the returned aligned buffer.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut bytes,
            )
        };
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        if bytes < std::mem::size_of::<TOKEN_USER>() as u32
            || unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    bytes,
                    &mut bytes,
                )
            } == 0
        {
            return Err(format!(
                "Cannot read the service account SID: {}",
                std::io::Error::last_os_error()
            ));
        }
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        (buffer, sid)
    };
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut text = std::ptr::null_mut();
    // SAFETY: sid references buffer, and the converted string is released with LocalFree.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(format!(
            "Cannot format the service account SID: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut length = 0;
    unsafe {
        while *text.add(length) != 0 {
            length += 1;
        }
    }
    let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(text.cast());
    }
    drop(buffer);
    Ok(result)
}
