//! Atomic replacement while readers retain a handle to the previous record generation.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_RENAME_INFO, FILE_RENAME_INFO_0, FileRenameInfoEx, SetFileInformationByHandle,
};

pub(super) fn temporary(parent: &Path) -> io::Result<tempfile::NamedTempFile<File>> {
    tempfile::Builder::new().make_in(parent, |path| {
        OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .open(path)
    })
}

pub(super) fn publish(mut temporary: tempfile::NamedTempFile<File>, path: &Path) -> io::Result<()> {
    let name: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if unit == b'/' as u16 {
                b'\\' as u16
            } else {
                unit
            }
        })
        .collect();
    let bytes = std::mem::size_of::<FILE_RENAME_INFO>() + name.len() * std::mem::size_of::<u16>();
    // u64 storage satisfies FILE_RENAME_INFO alignment on Windows x64 and ARM64.
    let mut storage = vec![0_u64; bytes.div_ceil(std::mem::size_of::<u64>())];
    let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // These flags preserve open readers of the previous generation. MoveFileEx alone
    // rejects that legitimate overlap with ERROR_ACCESS_DENIED on Windows (2026-10-02).
    const REPLACE_IF_EXISTS: u32 = 1;
    const POSIX_SEMANTICS: u32 = 2;
    // SAFETY: storage is aligned and large enough for the header plus the entire name;
    // the source handle has DELETE access and both buffers live through the system call.
    let renamed = unsafe {
        (*information).Anonymous = FILE_RENAME_INFO_0 {
            Flags: REPLACE_IF_EXISTS | POSIX_SEMANTICS,
        };
        (*information).FileNameLength = (name.len() * std::mem::size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast(),
            name.len(),
        );
        SetFileInformationByHandle(
            temporary.as_file().as_raw_handle().cast(),
            FileRenameInfoEx,
            information.cast(),
            bytes as u32,
        )
    };
    if renamed == 0 {
        return Err(io::Error::new(
            io::Error::last_os_error().kind(),
            format!(
                "cannot atomically publish {}: {}",
                path.display(),
                io::Error::last_os_error()
            ),
        ));
    }
    temporary.disable_cleanup(true);
    Ok(())
}
