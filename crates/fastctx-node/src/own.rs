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
    if cfg!(windows) && fs::read(directory.join("fastctx")).ok().is_none_or(|held| held != GIT_BASH_SHIM.as_bytes()) {
        publish(&directory.join("fastctx"), GIT_BASH_SHIM.as_bytes(), Permissions::Private)?;
    }
    Ok((copy, remove_others(world, &programs, &name)))
}

/// What Git Bash runs for `fastctx` in the World's shell on Windows. Before
/// it starts a native program, Git Bash rewrites each argument that looks
/// like a Unix path into one under its own installation, and a path given
/// to fastctx is often another machine's (`fastctx journal add
/// --attach-from laptop /srv/run.log`). Only fastctx is started without
/// that rewriting; every other program sees the shell as Git Bash made it.
/// Git Bash finds this file before `fastctx.exe` beside it.
const GIT_BASH_SHIM: &str =
    "#!/bin/sh\nMSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' exec \"${0%/*}/fastctx.exe\" \"$@\"\n";

/// Removes other versions only after their jobs' complete process trees
/// have ended. Windows executable sharing alone does not express that
/// lifetime, and Linux permits unlinking a running program outright.
fn remove_others(world: &WorldDir, programs: &Path, keep: &str) -> Vec<(PathBuf, io::Error)> {
    let entries = match fs::read_dir(programs) {
        Ok(entries) => entries,
        Err(error) => return vec![(programs.to_path_buf(), error)],
    };
    let uses = fastctx_hands::run::program::uses(world);
    let mut kept = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                kept.push((programs.to_path_buf(), error));
                continue;
            }
        };
        let path = entry.path();
        if entry.file_name() == keep {
            continue;
        }
        let unused = match &uses {
            Ok(uses) => uses.iter().try_for_each(|usage| match usage.reason_for(&path)? {
                Some(reason) => Err(io::Error::new(io::ErrorKind::WouldBlock, reason)),
                None => Ok(()),
            }),
            Err(error) => Err(io::Error::new(error.kind(), format!("could not establish job usage: {error}"))),
        };
        if let Err(error) = unused.and_then(|()| fs::remove_dir_all(&path)) {
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
