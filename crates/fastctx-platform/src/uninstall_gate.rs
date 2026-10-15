//! An uninstall exclusion gate with no filesystem entry to retire. Removing
//! a lock file after unlocking would permit two locks on different objects.

use std::io;
use std::path::Path;

fn name(root: &Path) -> io::Result<String> {
    let path = if root.try_exists()? {
        std::fs::canonicalize(root)?
    } else {
        let parent = root.parent().ok_or_else(|| io::Error::other("the World root has no parent"))?;
        std::fs::canonicalize(parent)?
            .join(root.file_name().ok_or_else(|| io::Error::other("the World root has no name"))?)
    };
    let path = path.to_string_lossy().replace('\\', "/");
    let path = if cfg!(windows) { path.trim_start_matches("//?/").to_lowercase() } else { path };
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    #[cfg(windows)]
    let user = crate::current_user_sid()?;
    #[cfg(unix)]
    // SAFETY: getuid cannot fail.
    let user = unsafe { libc::getuid() }.to_string();
    Ok(format!("fastctx-uninstall-{user}-{hash:016x}"))
}

/// macOS has no abstract socket names: the gate is a lock on a file in the
/// user's temporary directory that is never removed, released whenever its
/// holder ends.
#[cfg(target_os = "macos")]
mod imp {
    use std::io;
    use std::os::fd::AsRawFd as _;
    pub struct Gate {
        _file: std::fs::File,
    }
    pub fn acquire(name: &str) -> io::Result<Option<Gate>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(std::env::temp_dir().join(format!("{name}.lock")))?;
        // SAFETY: a lock on a descriptor owned here.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(error) };
        }
        Ok(Some(Gate { _file: file }))
    }
}

pub struct Gate {
    _gate: imp::Gate,
}

impl Gate {
    pub fn try_acquire(root: &Path) -> io::Result<Option<Self>> {
        imp::acquire(&name(root)?).map(|gate| gate.map(|gate| Self { _gate: gate }))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};

    pub struct Gate {
        _pipe: OwnedHandle,
    }
    pub fn acquire(name: &str) -> io::Result<Option<Gate>> {
        let name: Vec<u16> = format!("\\\\.\\pipe\\{name}").encode_utf16().chain([0]).collect();
        let descriptor = crate::windows_user::OwnerOnly::new(false)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.descriptor(),
            bInheritHandle: 0,
        };
        // SAFETY: strings, descriptor and attributes remain live. The pipe
        // is an owned, first-instance kernel object and receives no clients.
        let pipe = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                0,
                0,
                &attributes,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) { Ok(None) } else { Err(error) };
        }
        Ok(Some(Gate { _pipe: unsafe { OwnedHandle::from_raw_handle(pipe) } }))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::os::fd::{FromRawFd as _, OwnedFd};
    pub struct Gate {
        _socket: OwnedFd,
    }
    pub fn acquire(name: &str) -> io::Result<Option<Gate>> {
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as _;
        if name.len() + 1 > address.sun_path.len() {
            return Err(io::Error::other("the uninstall gate name exceeds the native socket name"));
        }
        for (target, byte) in address.sun_path[1..].iter_mut().zip(name.bytes()) {
            *target = byte as libc::c_char;
        }
        let size = std::mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + name.len();
        if unsafe { libc::bind(fd, (&raw const address).cast(), size as libc::socklen_t) } == -1 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EADDRINUSE) { Ok(None) } else { Err(error) };
        }
        Ok(Some(Gate { _socket: socket }))
    }
}
