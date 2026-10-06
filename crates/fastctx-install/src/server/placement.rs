//! Idempotent durable placement of an already verified release package.

use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};
use sha2::{Digest as _, Sha256};

/// Places the calling release under `root/bin`, retaining its notices under
/// `root/licenses`. The caller serializes installation with its own lock.
/// A different installed image is left untouched: changing an existing node's
/// version belongs to the World's renewal operation.
pub fn place(root: &Path, source: &Path) -> anyhow::Result<PathBuf> {
    ensure!(root.is_absolute() && source.is_absolute(), "server installation paths must be absolute");
    crate::disk::ensure_private_dir(root)?;
    let bin = root.join("bin");
    crate::disk::ensure_private_dir(&bin)?;
    let installed = crate::executable::installed_path(root);
    let expected = fingerprint(source)?;
    if installed.try_exists()? {
        crate::disk::ordinary(&fs::symlink_metadata(&installed)?)?;
        ensure!(
            fingerprint(&installed)? == expected,
            "{} contains a different kernel; use World's version update to replace it",
            installed.display()
        );
    }
    let licenses = root.join("licenses");
    let package = if installed.try_exists()? && fs::canonicalize(source)? == fs::canonicalize(&installed)? {
        licenses.clone()
    } else {
        source.parent().context("the release executable has no package directory")?.to_path_buf()
    };
    let documents = ["LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md"]
        .map(|name| fs::read(package.join(name)).with_context(|| format!("reading release notice {name}")));
    let [apache, notice, dependencies] = documents;
    let documents = [apache?, notice?, dependencies?];
    crate::disk::ensure_private_dir(&licenses)?;
    for (name, bytes) in ["LICENSE-APACHE", "NOTICE", "THIRD_PARTY_LICENSES.md"].into_iter().zip(documents) {
        let destination = licenses.join(name);
        if read_if_present(&destination)?.as_deref() != Some(&bytes) {
            let mut staged = crate::disk::Staged::create(&destination, 0o600)?;
            staged.file.write_all(&bytes)?;
            staged.file.sync_all()?;
            fs::rename(&staged.path, &destination)?;
            staged.present = false;
            crate::disk::sync_parent(&destination)?;
        }
    }
    if !installed.try_exists()? {
        let mut input = File::open(source)?;
        let mut output = crate::disk::Staged::create(&installed, 0o755)?;
        let mut buffer = [0; 64 * 1024];
        let mut copied = Sha256::new();
        loop {
            let length = input.read(&mut buffer)?;
            if length == 0 {
                break;
            }
            copied.update(&buffer[..length]);
            output.file.write_all(&buffer[..length])?;
        }
        ensure!(<[u8; 32]>::from(copied.finalize()) == expected, "the source image changed while it was copied");
        output.file.sync_all()?;
        // Publishing a new hard link cannot replace a file created by another
        // writer. Random staging names also survive PID reuse after a crash.
        fs::hard_link(&output.path, &installed)?;
        crate::disk::sync_parent(&installed)?;
        output.discard()?;
    }
    crate::disk::sync_directory(root)?;
    Ok(installed)
}

fn fingerprint(path: &Path) -> anyhow::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            return Ok(hash.finalize().into());
        }
        hash.update(&buffer[..length]);
    }
}

fn read_if_present(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
