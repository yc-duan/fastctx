//! Public release files are opened without following links. Directory
//! handles keep each lookup inside the hub even while names are replaced.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub struct Files {
    pub root: PathBuf,
    #[cfg(unix)]
    directory: File,
}

impl Files {
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        #[cfg(unix)]
        let directory = {
            use std::os::unix::fs::OpenOptionsExt as _;
            std::fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(&root)?
        };
        Ok(Self {
            root,
            #[cfg(unix)]
            directory,
        })
    }

    pub fn open(&self, version: &str, name: &str) -> io::Result<File> {
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::os::fd::{AsRawFd as _, FromRawFd as _};
            use std::os::unix::fs::MetadataExt as _;

            fn at(parent: &File, name: &str, flags: i32) -> io::Result<File> {
                let name = CString::new(name)?;
                // SAFETY: the parent and NUL-terminated component remain
                // alive; a successful descriptor is transferred once.
                let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags | libc::O_CLOEXEC) };
                if fd == -1 {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: openat returned an owned descriptor.
                Ok(unsafe { File::from_raw_fd(fd) })
            }
            let directory = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;
            let releases = at(&self.directory, "releases", directory)?;
            let version = at(&releases, &format!("v{version}"), directory)?;
            let file = at(&version, name, libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK)?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a published release must be a regular file with one link",
                ));
            }
            Ok(file)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
            use std::os::windows::io::AsRawHandle as _;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
            };

            fn open(path: &Path) -> io::Result<File> {
                // Denying delete sharing pins directories against a rename
                // or junction replacement until the leaf has been opened.
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                    .open(path)
            }
            let mut path = self.root.clone();
            let mut directories = Vec::new();
            for component in [None, Some("releases"), Some(&format!("v{version}"))] {
                if let Some(component) = component {
                    path.push(component);
                }
                let directory = open(&path)?;
                let metadata = directory.metadata()?;
                if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "a release directory cannot be a link"));
                }
                directories.push(directory);
            }
            let file = open(&path.join(name))?;
            let metadata = file.metadata()?;
            if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a published release must be a regular file"));
            }
            // SAFETY: the output structure is valid zero-initialized storage
            // and file owns a live handle throughout the call.
            let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if information.nNumberOfLinks != 1 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a published release cannot be a hard link"));
            }
            Ok(file)
        }
    }
}
