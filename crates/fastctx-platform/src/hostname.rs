//! The machine's own name for itself.

use std::io;

/// The host name the system reports.
pub fn hostname() -> io::Result<String> {
    imp::hostname()
}

#[cfg(unix)]
mod imp {
    use std::io;

    pub fn hostname() -> io::Result<String> {
        let mut buffer = vec![0u8; 256];
        // SAFETY: the buffer holds its length in bytes.
        if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
        String::from_utf8(buffer[..end].to_vec()).map_err(|_| io::Error::other("the host name is not UTF-8"))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;

    use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};

    pub fn hostname() -> io::Result<String> {
        let mut length = 0u32;
        // SAFETY: a size query with no buffer.
        unsafe { GetComputerNameExW(ComputerNameDnsHostname, std::ptr::null_mut(), &mut length) };
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: the buffer holds `length` units.
        if unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err(io::Error::last_os_error());
        }
        String::from_utf16(&buffer[..length as usize]).map_err(|_| io::Error::other("the host name is not Unicode"))
    }
}
