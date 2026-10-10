use std::path::{Path, PathBuf};
use std::{fs, io};

/// Deletes one checked directory entry, including a mapped Windows image.
/// A retained hard link outside the installation lets a running CLI's old
/// image lose its installed name without deleting the executing object.
pub fn delete_file(path: &Path) -> io::Result<()> {
    super::checked_path(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if crate::disk::is_link(&metadata) {
        #[cfg(windows)]
        let directory = {
            use std::os::windows::fs::MetadataExt as _;
            // A directory link has a link file type but keeps its directory attribute.
            metadata.file_attributes() & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY != 0
        };
        #[cfg(not(windows))]
        let directory = metadata.is_dir();
        return if directory { fs::remove_dir(path) } else { fs::remove_file(path) };
    }
    #[cfg(unix)]
    {
        // A socket a stopped node left behind cannot be opened (ENXIO), nor
        // a FIFO without blocking; neither holds data, so its name goes.
        use std::os::unix::fs::FileTypeExt as _;
        let kind = metadata.file_type();
        if kind.is_socket() || kind.is_fifo() {
            return fs::remove_file(path);
        }
    }
    let file = crate::disk::open(path)?;
    crate::disk::delete_owned(&file, path)
}

/// Removes an explicitly selected product data tree without following links.
/// Mounted or redirected parents are rejected before walking descendants.
/// Every entry is tried; failures identify the paths that remain for retry.
pub fn delete_tree(path: &Path) -> io::Result<()> {
    let mut failed = Vec::new();
    if let Err(error) = delete_tree_entry(path, &mut failed) {
        failed.push((path.to_path_buf(), error));
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(
            failed.iter().map(|(path, error)| format!("{}: {error}", path.display())).collect::<Vec<_>>().join("; "),
        ))
    }
}

fn delete_tree_entry(path: &Path, failed: &mut Vec<(PathBuf, io::Error)>) -> io::Result<()> {
    super::checked_path(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if crate::disk::is_link(&metadata) {
        return delete_file(path);
    }
    if !metadata.is_dir() {
        return delete_file(path);
    }
    let before = failed.len();
    for entry in fs::read_dir(path)? {
        match entry {
            Ok(entry) => {
                if let Err(error) = delete_tree_entry(&entry.path(), failed) {
                    failed.push((entry.path(), error));
                }
            }
            Err(error) => failed.push((path.to_path_buf(), error)),
        }
    }
    if failed.len() == before {
        fs::remove_dir(path)?;
    }
    Ok(())
}

/// What retiring the program directories left: additions that are not
/// fastctx's, and owned files that could not be removed, each with why.
pub struct Programs {
    pub left: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, io::Error)>,
}

/// Retires executable assets while leaving arbitrary additions to bin intact.
/// Every owned file is tried; one that cannot go does not keep the rest.
pub fn remove_programs(root: &Path) -> Programs {
    let mut programs = Programs { left: Vec::new(), failed: Vec::new() };
    for bin in [root.join("bin"), root.join("world/bin"), root.join("renewal/images")] {
        if let Err(error) = retire_bin(&bin, &mut programs) {
            programs.failed.push((bin, error));
        }
    }
    programs
}

fn retire_bin(bin: &Path, programs: &mut Programs) -> io::Result<()> {
    super::checked_path(bin)?;
    match fs::symlink_metadata(bin) {
        Ok(metadata) if crate::disk::is_link(&metadata) => {
            programs.left.push(bin.to_path_buf());
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        _ => {}
    }
    let entries = match fs::read_dir(bin) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if let Err(error) = retire_entry(&path, programs) {
            programs.failed.push((path, error));
        }
    }
    if fs::read_dir(bin)?.next().is_none() {
        fs::remove_dir(bin)?;
    }
    Ok(())
}

fn retire_entry(path: &Path, programs: &mut Programs) -> io::Result<()> {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir()
        && !crate::disk::is_link(&metadata)
        && matches!(name.len(), 24 | 64)
        && name.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        retire_bin(path, programs)
    } else if name.starts_with(".fastctx-exe-retired-") {
        crate::executable::delete_retired(path).map_err(|error| io::Error::other(error.to_string()))
    } else if !metadata.is_dir()
        && matches!(
            name,
            "fastctx"
                | "fastctx.exe"
                | "rg"
                | "rg.exe"
                | "prelude.bash"
                | "state.bash"
                | "LICENSE-APACHE"
                | "NOTICE"
                | "THIRD_PARTY_LICENSES.md"
                | "fastctx.renewal-files.json"
        )
    {
        delete_file(path)
    } else {
        programs.left.push(path.to_path_buf());
        Ok(())
    }
}
