//! Reading the Windows registry.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt as _;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};

#[derive(Clone, Copy, Debug)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
}

fn root(hive: Hive) -> HKEY {
    match hive {
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        Hive::CurrentUser => HKEY_CURRENT_USER,
    }
}

/// The 32-bit value `value` of `key` in `hive`, or `None` if the key or the
/// value does not exist.
pub fn dword_value(hive: Hive, key: &str, value: &str) -> io::Result<Option<u32>> {
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let mut data = 0u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: NUL-terminated names; the value is four bytes.
    let code = unsafe {
        RegGetValueW(
            root(hive),
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut data).cast(),
            &mut bytes,
        )
    };
    match code {
        ERROR_SUCCESS => Ok(Some(data)),
        ERROR_FILE_NOT_FOUND => Ok(None),
        code => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

/// The string value `value` of `key` in `hive`, or `None` if the key or the
/// value does not exist.
pub fn string_value(hive: Hive, key: &str, value: &str) -> io::Result<Option<OsString>> {
    let root = root(hive);
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let mut bytes = 0u32;
    loop {
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        let pointer = if buffer.is_empty() { std::ptr::null_mut() } else { buffer.as_mut_ptr().cast() };
        // SAFETY: NUL-terminated names; the buffer holds `bytes` bytes.
        let code = unsafe {
            RegGetValueW(root, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), pointer, &mut bytes)
        };
        match code {
            ERROR_SUCCESS if !buffer.is_empty() => {
                let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
                return Ok(Some(OsString::from_wide(&buffer[..end])));
            }
            // The first call only asked for the size.
            ERROR_SUCCESS | ERROR_MORE_DATA => continue,
            ERROR_FILE_NOT_FOUND => return Ok(None),
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}
