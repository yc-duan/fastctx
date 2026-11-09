use super::{FileIdentity, Name, invalid};
use std::{
    ffi::CString,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};
fn c_name(name: &Name) -> CString {
    CString::new(name.as_os_str().as_bytes()).expect("validated component")
}
fn owned(fd: i32) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: a successful open returned a new descriptor owned here.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
pub(super) fn absolute_components(path: &Path) -> io::Result<(PathBuf, Vec<Name>)> {
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(v) => names.push(Name::new(v)?),
            _ => return Err(invalid("root path must be normalized")),
        }
    }
    Ok((PathBuf::from("/"), names))
}
pub(super) fn open_anchor(_: &Path) -> io::Result<File> {
    // SAFETY: a fixed terminated path; the descriptor is returned as owned.
    owned(unsafe { libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW) })
}
pub(super) fn open_directory(parent: &File, name: &Name, create: bool) -> io::Result<File> {
    let name = c_name(name);
    if create {
        // SAFETY: the live parent descriptor and single terminated component.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
    }
    // SAFETY: O_NOFOLLOW rejects replacement by a link at the opened component.
    owned(unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })
}
pub(super) fn check_private(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed root must belong only to the process user",
        ));
    }
    Ok(())
}
pub(super) fn identity(file: &File) -> io::Result<FileIdentity> {
    let m = file.metadata()?;
    Ok(FileIdentity { volume: m.dev(), file: m.ino() })
}
pub(super) fn sync_directory(file: &File) -> io::Result<()> {
    file.sync_all()
}
pub(super) fn open_file(parent: &File, name: &Name, create: bool, write: bool) -> io::Result<File> {
    let name = c_name(name);
    let flags = libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if write { libc::O_RDWR } else { libc::O_RDONLY }
        | if create { libc::O_CREAT | libc::O_EXCL } else { 0 };
    // SAFETY: live directory descriptor, checked component, private initial mode.
    let file = owned(unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, 0o600) })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("managed content must be a plain file with one link"));
    }
    Ok(file)
}
pub(super) fn set_mode(file: &File, mode: u32) -> io::Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(mode))
}
pub(super) fn rename_file(parent: &File, from: &Name, _file: &File, to: &Name, replace: bool) -> io::Result<()> {
    if !replace {
        return rename_exclusive(parent, from, to);
    }
    let from = c_name(from);
    let to = c_name(to);
    // SAFETY: both names are single components in the same held parent.
    if unsafe { libc::renameat(parent.as_raw_fd(), from.as_ptr(), parent.as_raw_fd(), to.as_ptr()) } != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn rename_flags(parent: &File, from: &Name, to: &Name, flags: u32) -> io::Result<()> {
    let from = c_name(from);
    let to = c_name(to);
    // SAFETY: same held parent and terminated names; no fallback opens a gap.
    if unsafe {
        libc::syscall(libc::SYS_renameat2, parent.as_raw_fd(), from.as_ptr(), parent.as_raw_fd(), to.as_ptr(), flags)
    } != 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(target_os = "macos")]
fn rename_flags(parent: &File, from: &Name, to: &Name, flags: u32) -> io::Result<()> {
    let from = c_name(from);
    let to = c_name(to);
    // SAFETY: same held parent and terminated names.
    if unsafe { libc::renameatx_np(parent.as_raw_fd(), from.as_ptr(), parent.as_raw_fd(), to.as_ptr(), flags) } != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn rename_exclusive(parent: &File, from: &Name, to: &Name) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rename_flags(parent, from, to, libc::RENAME_NOREPLACE)
    }
    #[cfg(target_os = "macos")]
    {
        rename_flags(parent, from, to, libc::RENAME_EXCL)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (parent, from, to);
        Err(io::Error::new(io::ErrorKind::Unsupported, "exclusive rename unavailable"))
    }
}
pub(super) fn exchange(parent: &File, left: &Name, right: &Name) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rename_flags(parent, left, right, libc::RENAME_EXCHANGE)
    }
    #[cfg(target_os = "macos")]
    {
        rename_flags(parent, left, right, libc::RENAME_SWAP)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (parent, left, right);
        Err(io::Error::new(io::ErrorKind::Unsupported, "atomic directory exchange unavailable"))
    }
}
pub(super) fn install_directory(parent: &File, from: &Name, to: &Name) -> io::Result<()> {
    rename_exclusive(parent, from, to)
}
pub(super) fn open_prepared(parent: &File, name: &Name) -> io::Result<File> {
    open_file(parent, name, false, false)
}
pub(super) fn confirm_prepared(file: &File) -> io::Result<()> {
    file.sync_all()
}
