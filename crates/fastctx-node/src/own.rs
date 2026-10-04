//! The copy of this program that everything the node starts runs from.
//!
//! The executable the node was started from need not stay where it is: an
//! old fastctx's upgrader runs the new version through `npm exec` and then
//! deletes the cache it ran from, and an install replaces a program in place.
//! What the node starts from itself (each job's supervisor, the World's tools
//! in every session, `fastctx` in the World's shell) must be this very
//! version for as long as the node runs. So the node first copies itself
//! under the World, in a directory named by the copy's contents, and starts
//! everything from there. The stable copy `fastctx apply` places for the
//! hosts is a different file and is left to it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::layout::WorldDir;
use sha2::{Digest as _, Sha256};

const NAME: &str = if cfg!(windows) { "fastctx.exe" } else { "fastctx" };

/// This program's copy under `world`, written when it is not there yet,
/// and the copies of other versions that could not be removed, with why.
pub fn pin(world: &WorldDir) -> io::Result<(PathBuf, Vec<(PathBuf, io::Error)>)> {
    let bytes = fs::read(std::env::current_exe()?)?;
    let digest = Sha256::digest(&bytes);
    let name: String = digest[..12].iter().map(|byte| format!("{byte:02x}")).collect();
    let programs = world.path().join("bin");
    let directory = programs.join(&name);
    let copy = directory.join(NAME);
    if fs::read(&copy).ok().is_none_or(|held| Sha256::digest(&held) != digest) {
        ensure_private_dir(&directory)?;
        publish(&copy, &bytes, Permissions::Private)?;
        executable(&copy)?;
    }
    Ok((copy, remove_others(&programs, &name)))
}

/// Removes the copies of other versions. One still running, as a job's
/// supervisor started by an older node, cannot be removed on Windows; it
/// stays, and is tried again when the node next starts.
fn remove_others(programs: &Path, keep: &str) -> Vec<(PathBuf, io::Error)> {
    let Ok(entries) = fs::read_dir(programs) else { return Vec::new() };
    let mut kept = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() != keep
            && let Err(error) = fs::remove_dir_all(&path)
        {
            kept.push((path, error));
        }
    }
    kept
}

#[cfg(unix)]
fn executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn executable(_path: &Path) -> io::Result<()> {
    Ok(())
}
