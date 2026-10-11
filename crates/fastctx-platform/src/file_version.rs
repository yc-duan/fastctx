//! The version a program file says it is, read from the file without
//! running it: on Windows, the version information a program carries among
//! its resources. Other systems have no such record in their programs.

use std::io;
use std::path::Path;

/// The product version `path` records, as its maker wrote it (`2.1.296`);
/// `None` when it records none.
pub fn file_version(path: &Path) -> io::Result<Option<String>> {
    imp::file_version(path)
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{
        ERROR_RESOURCE_DATA_NOT_FOUND, ERROR_RESOURCE_NAME_NOT_FOUND, ERROR_RESOURCE_TYPE_NOT_FOUND,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };

    pub fn file_version(path: &Path) -> io::Result<Option<String>> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut ignored = 0u32;
        // SAFETY: a NUL-terminated path; the handle argument is ignored.
        let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut ignored) };
        if size == 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                // A file without version information has no such resource.
                Some(code)
                    if [
                        ERROR_RESOURCE_DATA_NOT_FOUND,
                        ERROR_RESOURCE_TYPE_NOT_FOUND,
                        ERROR_RESOURCE_NAME_NOT_FOUND,
                    ]
                    .contains(&(code as u32)) =>
                {
                    Ok(None)
                }
                _ => Err(error),
            };
        }
        let mut data = vec![0u8; size as usize];
        // SAFETY: the buffer is as large as the system said.
        if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some(text) = product_string(&data) {
            return Ok(Some(text));
        }
        let root: Vec<u16> = "\\".encode_utf16().chain([0]).collect();
        let mut fixed: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        // SAFETY: `data` is the block GetFileVersionInfoW filled.
        if unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut fixed, &mut length) } == 0
            || (length as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            return Ok(None);
        }
        // SAFETY: the system points into `data` at a whole VS_FIXEDFILEINFO.
        let info = unsafe { &*(fixed as *const VS_FIXEDFILEINFO) };
        Ok(Some(format!(
            "{}.{}.{}.{}",
            info.dwProductVersionMS >> 16,
            info.dwProductVersionMS & 0xffff,
            info.dwProductVersionLS >> 16,
            info.dwProductVersionLS & 0xffff
        )))
    }

    /// The `ProductVersion` string of the block's first language.
    fn product_string(data: &[u8]) -> Option<String> {
        let query: Vec<u16> = "\\VarFileInfo\\Translation".encode_utf16().chain([0]).collect();
        let mut pointer: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut length = 0u32;
        // SAFETY: `data` is a filled version block; the query is NUL-terminated.
        if unsafe { VerQueryValueW(data.as_ptr().cast(), query.as_ptr(), &mut pointer, &mut length) } == 0 || length < 4
        {
            return None;
        }
        // SAFETY: the translation table is pairs of u16 inside `data`.
        let (language, code_page) = unsafe { (*(pointer as *const u16), *((pointer as *const u16).add(1))) };
        let key = format!("\\StringFileInfo\\{language:04x}{code_page:04x}\\ProductVersion");
        let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
        let mut text: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: as above; the answer is a count of UTF-16 units.
        if unsafe { VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut text, &mut length) } == 0 || length == 0 {
            return None;
        }
        // SAFETY: `length` units of text inside `data`, NUL included.
        let units = unsafe { std::slice::from_raw_parts(text as *const u16, length as usize) };
        let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
        let value = String::from_utf16_lossy(&units[..end]).trim().to_string();
        (!value.is_empty()).then_some(value)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;
    use std::path::Path;

    pub fn file_version(_path: &Path) -> io::Result<Option<String>> {
        Ok(None)
    }
}
