//! Rebuild read-only files without following links planted in a replica.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use fastctx_platform::atomic::Staged;
use sha2::{Digest, Sha256};

pub(crate) enum Content {
    Bytes(Vec<u8>),
    File { source: PathBuf, size: u64, hash: String },
}

impl Content {
    fn matches(&self, path: &Path) -> io::Result<bool> {
        match self {
            Self::Bytes(bytes) => Ok(std::fs::read(path)? == *bytes),
            Self::File { size, hash, .. } => Ok(fingerprint(path)? == (*size, hash.clone())),
        }
    }

    fn publish(&self, path: &Path) -> io::Result<()> {
        let mut staged = Staged::create(path, Some(0o600))?;
        match self {
            Self::Bytes(bytes) => staged.write_all(bytes)?,
            Self::File { source, size, hash } => {
                if !std::fs::symlink_metadata(source)?.file_type().is_file() {
                    return Err(io::Error::other(fastctx_types::memory::words::EVIDENCE_NOT_FILE));
                }
                let mut source = std::fs::File::open(source)?;
                let mut digest = Sha256::new();
                let mut count = 0;
                let mut buffer = [0; 64 * 1024];
                loop {
                    let read = source.read(&mut buffer)?;
                    if read == 0 {
                        break;
                    }
                    count += read as u64;
                    digest.update(&buffer[..read]);
                    staged.write_all(&buffer[..read])?;
                }
                if count != *size || format!("{:x}", digest.finalize()) != *hash {
                    return Err(io::Error::other(fastctx_types::memory::words::EVIDENCE_CHANGED));
                }
            }
        }
        staged.commit()?;
        readonly(path)
    }
}

pub(crate) fn fingerprint(path: &Path) -> io::Result<(u64, String)> {
    let mut source = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok((size, format!("{:x}", hash.finalize())))
}

/// Returns relative paths repaired. Unexpected files and links are moved aside intact.
pub(crate) fn reconcile(root: &Path, expected: &BTreeMap<String, Content>, archive: &Path) -> io::Result<Vec<String>> {
    let mut changed = Vec::new();
    directory(root, archive, &mut changed, "")?;
    sweep(root, root, expected, archive, &mut changed)?;
    for (relative, content) in expected {
        let path = root.join(relative);
        let mut parent = root.to_path_buf();
        let components: Vec<_> = Path::new(relative).components().collect();
        for component in &components[..components.len() - 1] {
            parent.push(component.as_os_str());
            directory(&parent, archive, &mut changed, relative)?;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() && content.matches(&path)? => readonly(&path)?,
            Ok(metadata) if metadata.file_type().is_file() => {
                std::fs::create_dir_all(archive)?;
                std::fs::copy(&path, archive.join(format!("{:06}", changed.len())))?;
                writable(&path)?;
                content.publish(&path)?;
                changed.push(relative.clone());
            }
            Ok(_) => {
                keep(&path, archive, &mut changed, relative)?;
                content.publish(&path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                content.publish(&path)?;
                changed.push(relative.clone());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(changed)
}

fn sweep(
    root: &Path,
    at: &Path,
    expected: &BTreeMap<String, Content>,
    archive: &Path,
    changed: &mut Vec<String>,
) -> io::Result<()> {
    for item in std::fs::read_dir(at)? {
        let path = item?.path();
        let relative = path.strip_prefix(root).map_err(io::Error::other)?.to_string_lossy().replace('\\', "/");
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() && expected.keys().any(|name| name.starts_with(&format!("{relative}/"))) {
            sweep(root, &path, expected, archive, changed)?;
        } else if !(metadata.file_type().is_file() && expected.contains_key(&relative)) {
            keep(&path, archive, changed, &relative)?;
        }
    }
    Ok(())
}

fn directory(path: &Path, archive: &Path, changed: &mut Vec<String>, relative: &str) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => {
            keep(path, archive, changed, relative)?;
            std::fs::create_dir(path)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::fs::create_dir_all(path),
        Err(error) => Err(error),
    }
}

fn keep(path: &Path, archive: &Path, changed: &mut Vec<String>, relative: &str) -> io::Result<()> {
    std::fs::create_dir_all(archive)?;
    let destination = archive.join(format!("{:06}", changed.len()));
    if std::fs::symlink_metadata(path)?.file_type().is_file() {
        writable(path)?;
    }
    std::fs::rename(path, destination)?;
    changed.push(relative.to_string());
    Ok(())
}

fn writable(path: &Path) -> io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions)
}

pub(crate) fn readonly(path: &Path) -> io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}
