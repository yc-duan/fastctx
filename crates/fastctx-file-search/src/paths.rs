use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points need not be symbolic links.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn roots(paths: Vec<String>) -> io::Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for text in paths {
        let path = fastctx_platform::local_path(text)?;
        if !path.is_absolute() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "file search roots must be absolute"));
        }
        validate_root(&path)?;
        let path = fs::canonicalize(path)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a file or directory: {}", path.display()),
            ));
        }
        roots.push(path);
    }
    roots.sort_by(|a, b| b.components().count().cmp(&a.components().count()).then(a.cmp(b)));
    roots.dedup();
    Ok(roots)
}

pub(crate) fn validate_root(path: &Path) -> io::Result<()> {
    for part in path.ancestors() {
        if linked(&fs::symlink_metadata(part)?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("search root traverses a link: {}", part.display()),
            ));
        }
    }
    Ok(())
}

pub(crate) fn excluded(path: &Path, root: &Path, is_dir: bool) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        let mut parts = relative.components().peekable();
        while let Some(part) = parts.next() {
            let Some(name) = part.as_os_str().to_str() else {
                continue;
            };
            let named =
                |expected: &str| if cfg!(windows) { name.eq_ignore_ascii_case(expected) } else { name == expected };
            if named(".git") || ((named("node_modules") || named("target")) && (is_dir || parts.peek().is_some())) {
                return true;
            }
        }
        false
    })
}

/// Recheck after ranking: a cached name may have been deleted or replaced
/// with a link, including a directory on the way to the file.
pub(crate) fn contained_file(
    path: &Path,
    root: &Path,
    checked: &mut std::collections::HashSet<PathBuf>,
) -> io::Result<bool> {
    for part in path.ancestors() {
        if part != path && checked.contains(part) {
            continue;
        }
        match fs::symlink_metadata(part) {
            Ok(metadata) if linked(&metadata) => return Ok(false),
            Ok(metadata) if part == path && !metadata.is_file() => return Ok(false),
            Ok(_) => {
                if part != path {
                    checked.insert(part.to_owned());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(fs::canonicalize(path)?.starts_with(root))
}

pub(crate) fn text(path: &Path) -> io::Result<String> {
    let text =
        path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a file name is not valid Unicode"))?;
    if cfg!(windows) {
        if let Some(network) = text.strip_prefix(r"\\?\UNC\") {
            Ok(format!("//{}", network.replace('\\', "/")))
        } else {
            Ok(text.strip_prefix(r"\\?\").unwrap_or(text).replace('\\', "/"))
        }
    } else {
        Ok(text.to_owned())
    }
}
