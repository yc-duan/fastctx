//! Never follow filesystem links while collecting owned scratch files.

use std::fs;
use std::io;
use std::path::Path;

pub fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        metadata.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    metadata.file_type().is_symlink()
}

/// Moves `source`, a file or a directory, to `target` as `rename` does, and
/// says from where to where when it cannot. On Windows a program holding
/// something inside a directory for a moment, a scanner or an indexer,
/// refuses the move: it is tried again for a second before that is said.
pub fn move_to(source: &Path, target: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(source, target) {
            Ok(()) => return Ok(()),
            // ERROR_ACCESS_DENIED and ERROR_SHARING_VIOLATION.
            Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)) && tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("moving {} to {}: {error}", source.display(), target.display()),
                ));
            }
        }
    }
}

/// Removes the directory `path` and all it holds, as `remove_dir_all` does.
/// On Windows what another program holds for a moment, or files still
/// being deleted, refuse it: it is tried again for a second or two.
pub fn remove_tree(path: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_DIR_NOT_EMPTY.
            Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 145)) && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Deletes owned names without traversing links, including Windows
/// directory junctions. A link's target survives collection.
pub fn delete(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if is_link(&metadata) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            if metadata.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY != 0 {
                return fs::remove_dir(path);
            }
        }
        return fs::remove_file(path);
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            delete(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}
