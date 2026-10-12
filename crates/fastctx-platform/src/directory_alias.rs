//! Directory aliases for paths already handed to a running program.

use std::io;
use std::path::Path;

/// Creates an alias without copying data. The destination must be absolute.
pub fn create(target: &Path, alias: &Path) -> io::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(alias) {
        if crate::owned_directory::is_link(&metadata) && std::fs::canonicalize(alias)? == std::fs::canonicalize(target)?
        {
            return Ok(());
        }
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the alias path is occupied"));
    }
    crate::ensure_private_dir(alias.parent().expect("alias parent"))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, alias)
    }
    #[cfg(windows)]
    {
        windows(target, alias)
    }
}

#[cfg(windows)]
fn windows(target: &Path, alias: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let absolute = std::fs::canonicalize(target)?;
    let shown: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    let text = absolute.to_string_lossy();
    let substitute: Vec<u16> = if let Some(rest) = text.strip_prefix("\\\\?\\") {
        format!("\\??\\{rest}").encode_utf16().collect()
    } else {
        format!("\\??\\{text}").encode_utf16().collect()
    };
    // Mount-point reparse data: two offsets, two lengths, then UTF-16 names.
    let names_bytes = (substitute.len() + shown.len() + 2) * 2;
    let data_len = u16::try_from(8 + names_bytes).map_err(|_| io::Error::other("alias path is too long"))?;
    let mut data = Vec::with_capacity(8 + usize::from(data_len));
    data.extend_from_slice(&0xa0000003u32.to_le_bytes());
    data.extend_from_slice(&data_len.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    for value in [0, (substitute.len() * 2) as u16, ((substitute.len() + 1) * 2) as u16, (shown.len() * 2) as u16] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for value in substitute.into_iter().chain([0]).chain(shown).chain([0]) {
        data.extend_from_slice(&value.to_le_bytes());
    }
    let staging = alias.parent().expect("alias parent").join(".aliases");
    crate::ensure_private_dir(&staging)?;
    let temporary = staging.join(alias.file_name().expect("alias name"));
    if let Ok(metadata) = std::fs::symlink_metadata(&temporary) {
        crate::owned_directory::delete(&temporary)?;
        if !metadata.is_dir() {
            return Err(io::Error::other("invalid staged directory alias"));
        }
    }
    std::fs::create_dir(&temporary)?;
    let name: Vec<u16> = temporary.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: the names and buffer live through synchronous Win32 calls.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    let result = if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        let mut returned = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                0x000900a4,
                data.as_ptr().cast(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        let result = if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) };
        unsafe {
            CloseHandle(handle);
        }
        result
    };
    if result.is_err() {
        std::fs::remove_dir(&temporary)?;
    }
    result?;
    std::fs::rename(&temporary, alias)?;
    // Concurrent alias publishers share this staging directory.
    Ok(())
}
