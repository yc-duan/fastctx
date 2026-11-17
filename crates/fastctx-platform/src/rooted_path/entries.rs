//! Bounded directory names from an independent cursor on the held directory.
//! Callers reopen children through Directory; an entry never carries a path to follow.
use std::{ffi::OsString, fs::File, io};
pub struct Entries(imp::Entries);
impl Entries {
    pub(super) fn open(file: File) -> io::Result<Self> {
        Ok(Self(imp::Entries::open(file)?))
    }
}
impl Iterator for Entries {
    type Item = io::Result<OsString>;
    fn next(&mut self) -> Option<Self::Item> {
        crate::roles::in_place("enumerate managed directory", || self.0.next())
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::{
        ffi::CStr,
        os::{fd::AsRawFd, unix::ffi::OsStringExt},
        ptr::NonNull,
    };
    pub(super) struct Entries {
        directory: NonNull<libc::DIR>,
        done: bool,
    }
    impl Entries {
        pub(super) fn open(file: File) -> io::Result<Self> {
            // A dup would share a cursor. Opening '.' relative to the capability
            // gives each iterator an independent open-file description.
            // SAFETY: the borrowed descriptor is live and the name is fixed.
            let fd = unsafe {
                libc::openat(
                    file.as_raw_fd(),
                    c".".as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: fdopendir takes ownership on success only.
            let directory = unsafe { libc::fdopendir(fd) };
            match NonNull::new(directory) {
                Some(directory) => Ok(Self { directory, done: false }),
                None => {
                    let error = io::Error::last_os_error();
                    unsafe { libc::close(fd) };
                    Err(error)
                }
            }
        }
        pub(super) fn next(&mut self) -> Option<io::Result<OsString>> {
            if self.done {
                return None;
            }
            loop {
                // SAFETY: errno is thread-local and readdir owns its result until
                // the next call on this cursor; copy the name before that call.
                unsafe {
                    *errno() = 0;
                    let entry = libc::readdir(self.directory.as_ptr());
                    if entry.is_null() {
                        self.done = true;
                        return if *errno() == 0 { None } else { Some(Err(io::Error::last_os_error())) };
                    }
                    let name = CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes();
                    if name == b"." || name == b".." {
                        continue;
                    }
                    return Some(Ok(OsString::from_vec(name.to_vec())));
                }
            }
        }
    }
    #[cfg(target_os = "android")]
    unsafe fn errno() -> *mut libc::c_int {
        unsafe { libc::__errno() }
    }
    #[cfg(any(target_os = "linux", target_os = "emscripten"))]
    unsafe fn errno() -> *mut libc::c_int {
        unsafe { libc::__errno_location() }
    }
    #[cfg(not(any(target_os = "android", target_os = "linux", target_os = "emscripten")))]
    unsafe fn errno() -> *mut libc::c_int {
        unsafe { libc::__error() }
    }
    // SAFETY: the stream has one owner and is never accessed concurrently;
    // POSIX directory streams may move between threads between calls.
    unsafe impl Send for Entries {}
    impl Drop for Entries {
        fn drop(&mut self) {
            // SAFETY: closes the stream and its owned descriptor exactly once.
            unsafe { libc::closedir(self.directory.as_ptr()) };
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::{ffi::OsStringExt, io::AsRawHandle},
        ptr,
    };
    use windows_sys::Win32::{Foundation::HANDLE, System::IO::IO_STATUS_BLOCK};
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryDirectoryFile(
            file: HANDLE,
            event: HANDLE,
            apc: *const c_void,
            context: *const c_void,
            status: *mut IO_STATUS_BLOCK,
            buffer: *mut c_void,
            length: u32,
            class: u32,
            single: u8,
            pattern: *const c_void,
            restart: u8,
        ) -> i32;
    }
    pub(super) struct Entries {
        file: File,
        buffer: Box<[u64; 1024]>,
        filled: usize,
        position: usize,
        done: bool,
    }
    impl Entries {
        pub(super) fn open(file: File) -> io::Result<Self> {
            Ok(Self { file, buffer: Box::new([0; 1024]), filled: 0, position: 0, done: false })
        }
        fn read_page(&mut self) -> io::Result<bool> {
            let mut ios = IO_STATUS_BLOCK::default();
            // SAFETY: synchronous owned handle and aligned 8 KiB output. Class
            // FileNamesInformation yields bounded linked entries, never bodies.
            let status = unsafe {
                NtQueryDirectoryFile(
                    self.file.as_raw_handle(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    &mut ios,
                    self.buffer.as_mut_ptr().cast(),
                    8192,
                    12,
                    0,
                    ptr::null(),
                    0,
                )
            };
            if status as u32 == 0x80000006 {
                return Ok(false);
            }
            super::super::windows::status(status)?;
            self.filled = ios.Information;
            self.position = 0;
            if self.filled == 0 || self.filled > 8192 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "directory page exceeds its bound"));
            }
            Ok(true)
        }
        fn item(&mut self) -> io::Result<OsString> {
            // SAFETY: only bytes reported inside the fixed output buffer are read.
            let bytes = unsafe { std::slice::from_raw_parts(self.buffer.as_ptr().cast::<u8>(), self.filled) };
            let at = self.position;
            if self.filled - at < 12 {
                return Err(bad());
            }
            let next = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let length = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
            if !length.is_multiple_of(2) || length > self.filled - at - 12 {
                return Err(bad());
            }
            if next != 0 && (next < 12 + length || next > self.filled - at) {
                return Err(bad());
            }
            self.position = if next == 0 { self.filled } else { at + next };
            let units = bytes[at + 12..at + 12 + length]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>();
            Ok(OsString::from_wide(&units))
        }
        pub(super) fn next(&mut self) -> Option<io::Result<OsString>> {
            if self.done {
                return None;
            }
            loop {
                if self.position == self.filled {
                    match self.read_page() {
                        Ok(true) => {}
                        Ok(false) => {
                            self.done = true;
                            return None;
                        }
                        Err(e) => {
                            self.done = true;
                            return Some(Err(e));
                        }
                    }
                }
                match self.item() {
                    Ok(name) if name == "." || name == ".." => continue,
                    Ok(name) => return Some(Ok(name)),
                    Err(e) => {
                        self.done = true;
                        return Some(Err(e));
                    }
                }
            }
        }
    }
    fn bad() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "malformed directory page")
    }
}
