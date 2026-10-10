//! Idempotent durable placement of an already verified release package.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::ensure;

/// Places the calling release under `root/bin`, retaining its notices under
/// `root/licenses`. The caller serializes installation with its own lock.
/// First placement and forward replacement use the same World renewal as
/// every other delivery route. Notices come from the selected stable image.
pub fn place(root: &Path, source: &Path) -> anyhow::Result<PathBuf> {
    ensure!(root.is_absolute() && source.is_absolute(), "server installation paths must be absolute");
    crate::disk::ensure_private_dir(root)?;
    let installed = crate::placement::ensure(root, source)?;
    let licenses = root.join("licenses");
    crate::disk::ensure_private_dir(&licenses)?;
    for (name, text) in crate::program::notices(&installed)? {
        let bytes = text.as_bytes();
        let destination = licenses.join(&name);
        if read_if_present(&destination)?.as_deref() != Some(bytes) {
            let mut staged = crate::disk::Staged::create(&destination, 0o600)?;
            staged.file.write_all(bytes)?;
            staged.file.sync_all()?;
            fs::rename(&staged.path, &destination)?;
            staged.present = false;
            crate::disk::sync_parent(&destination)?;
        }
    }
    crate::disk::sync_directory(root)?;
    Ok(installed)
}

fn read_if_present(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
