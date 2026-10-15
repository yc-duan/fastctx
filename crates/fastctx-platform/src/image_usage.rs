//! Hold the OS execution/write exclusion through collection, including
//! programs running through another hard-link name or under another user.

use std::fs::File;
#[cfg(any(windows, target_os = "linux"))]
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// No bytes are written. A write handle establishes that this image is
/// unmapped and prevents execution until the caller has unlinked it.
#[cfg(any(windows, target_os = "linux"))]
pub fn unused(path: &Path) -> io::Result<Option<File>> {
    let metadata = std::fs::symlink_metadata(path)?;
    if crate::owned_directory::is_link(&metadata) || !metadata.is_file() {
        return Err(io::Error::other(format!("unexpected program image {}", path.display())));
    }
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // A mapped image refuses FILE_WRITE_DATA. Deny new image readers,
        // but allow the caller to unlink the name while this handle lives.
        options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE);
    }
    match options.open(path) {
        Ok(file) => Ok(Some(file)),
        #[cfg(target_os = "linux")]
        Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => Ok(None),
        #[cfg(windows)]
        Err(error) if error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32) => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// macOS lets a running image be opened for writing, so no handle proves it
/// unused: no running process may have it, by its file's identity.
#[cfg(target_os = "macos")]
pub fn unused(path: &Path) -> io::Result<Option<File>> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(path)?;
    if crate::owned_directory::is_link(&metadata) || !metadata.is_file() {
        return Err(io::Error::other(format!("unexpected program image {}", path.display())));
    }
    for process in crate::darwin_proc::all()? {
        if let Some(image) = crate::darwin_proc::image(process.pid)
            && let Ok(running) = std::fs::metadata(&image)
            && running.dev() == metadata.dev()
            && running.ino() == metadata.ino()
        {
            return Ok(None);
        }
    }
    File::open(path).map(Some)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn unused(_path: &Path) -> io::Result<Option<File>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "image collection supports Windows and Linux"))
}
