//! File identity helpers shared with viewing. No resident state is stored here.

use sha2::{Digest, Sha256};
use std::io;
use std::path::{Component, Path, PathBuf};

/// Lowercase hexadecimal SHA-256 of the original bytes, including BOM and line endings.
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Fingerprints an unchanged file without keeping its contents in memory.
pub fn content_fingerprint(path: &Path) -> io::Result<fastctx_types::session::Fingerprint> {
    use std::io::Read as _;
    let mut file = fastctx_platform::file_read::open(path)?;
    let before = fastctx_platform::file_read::snapshot(&file, &file.metadata()?)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; crate::spool::BLOCK];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if fastctx_platform::file_read::snapshot(&file, &file.metadata()?)? != before {
        return Err(io::Error::other("the artifact changed while its content was fingerprinted"));
    }
    Ok(fastctx_types::session::Fingerprint { size: before.size, sha256: format!("{:x}", hash.finalize()) })
}

/// Resolves links and existing ancestors, also for a file not yet created.
/// Callers must use this same identity for the keys in `SeenFiles`.
pub fn canonical_path(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Use an absolute file path"));
    }
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Ok(metadata) = std::fs::symlink_metadata(path)
                && metadata.file_type().is_symlink()
            {
                let link = std::fs::read_link(path)?;
                return canonical_path(&path.parent().expect("absolute link has parent").join(link));
            }
            let parent = path.parent().ok_or(error)?;
            let mut parent = canonical_path(parent)?;
            match path.components().next_back() {
                Some(Component::Normal(name)) => parent.push(name),
                Some(Component::ParentDir) => {
                    parent.pop();
                }
                Some(Component::CurDir) => {}
                _ => {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "Path has no file name"));
                }
            }
            Ok(parent)
        }
        Err(error) => Err(error),
    }
}
