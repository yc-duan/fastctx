//! Publish a complete file with one same-directory rename.

use std::io::Write;
use std::path::Path;

pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("File path has no parent: {}", path.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        format!(
            "Cannot create temporary file in {}: {error}",
            parent.display()
        )
    })?;
    #[cfg(unix)]
    if let Some(mode) = crate::control::transaction::existing_unix_mode(path) {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|error| {
                format!("Cannot preserve permissions of {}: {error}", path.display())
            })?;
    }
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| {
            format!(
                "Cannot write temporary file for {}: {error}",
                path.display()
            )
        })?;
    #[cfg(windows)]
    rename_windows(&temporary, path)?;
    #[cfg(not(windows))]
    temporary.persist(path).map_err(|error| {
        format!(
            "Cannot atomically replace {}: {}",
            path.display(),
            error.error
        )
    })?;
    Ok(())
}

#[cfg(windows)]
fn rename_windows(temporary: &tempfile::NamedTempFile, path: &Path) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_NORMAL, FILE_RENAME_INFO, FileRenameInfoEx, SetFileAttributesW,
        SetFileInformationByHandle,
    };

    let rename_handle = OpenOptions::new()
        .access_mode(DELETE)
        .open(temporary.path())
        .map_err(|error| format!("Cannot open temporary file for rename: {error}"))?;
    let temporary_name: Vec<u16> = temporary
        .path()
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { SetFileAttributesW(temporary_name.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
        return Err(format!(
            "Cannot prepare temporary file for rename: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    let name_bytes = std::mem::size_of_val(name.as_slice());
    // The Win32 wrapper consumes a NUL-terminated name even though FileNameLength
    // counts only the bytes before the terminator.
    name.push(0);
    let buffer_bytes =
        std::mem::offset_of!(FILE_RENAME_INFO, FileName) + std::mem::size_of_val(name.as_slice());
    let length = u32::try_from(buffer_bytes)
        .map_err(|_| "Rename path exceeds the Windows API buffer size.".to_string())?;
    // Allocate complete structs to give the variable-length Win32 buffer its required alignment.
    let mut buffer = vec![
        FILE_RENAME_INFO::default();
        buffer_bytes.div_ceil(std::mem::size_of::<FILE_RENAME_INFO>())
    ];
    let info = buffer.as_mut_ptr();
    // POSIX rename keeps already-open readers on the old file while new readers
    // open the replacement. MoveFileExW cannot replace a destination held open.
    // Flag values: Microsoft FILE_RENAME_INFORMATION, REPLACE_IF_EXISTS and POSIX_SEMANTICS.
    unsafe {
        (*info).Anonymous.Flags = 0x00000001 | 0x00000002;
        (*info).FileNameLength = name_bytes as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            rename_handle.as_raw_handle() as _,
            FileRenameInfoEx,
            info.cast(),
            length,
        ) == 0
        {
            return Err(format!(
                "Cannot atomically replace {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}
