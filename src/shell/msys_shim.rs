//! A `fastctx` launcher for Git Bash on Windows.
//!
//! Git Bash (the MSYS2 runtime) rewrites arguments that look like POSIX paths before it starts
//! a native Windows program: `/root/app` becomes `C:/Program Files/Git/root/app`. A script that
//! reaches a Linux machine through `fastctx world run` or `fastctx world copy` names paths on
//! that machine, and they must arrive untouched. The shim is a shell script called `fastctx`
//! that turns the conversion off and starts the real binary; it goes on the command search
//! path in front of the binary's own directory, so `fastctx` from Bash runs the shim while
//! PowerShell and cmd, which ignore extensionless files, keep running the executable.

use crate::paths::display_path;
use std::path::{Path, PathBuf};

/// Directory of the shim under `~/.fastctx`, beside the managed binary.
const SHIM_DIRECTORY: &str = "bin";
/// Name the script must have: exactly the command a script types.
const SHIM_NAME: &str = "fastctx";

/// Writes the shim for the running binary when it is missing or names another binary, and
/// returns its directory. `None` on other platforms, when the running program is not named
/// fastctx, or when the directory cannot be written;
/// the binary's own directory is still on the path then, so `fastctx` keeps resolving and only
/// the Bash path rewriting stays in effect.
pub(crate) fn ensure(fastctx_dir: &Path) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    // The launcher starts fastctx. A program of another name that embeds this code, such as a
    // test harness, would point the user's `fastctx` command at itself.
    if !exe.file_stem().is_some_and(|stem| stem.eq_ignore_ascii_case(SHIM_NAME)) {
        return None;
    }
    let directory = fastctx_dir.join(SHIM_DIRECTORY);
    let shim = directory.join(SHIM_NAME);
    let wanted = content(&exe);
    if std::fs::read(&shim).is_ok_and(|current| current == wanted.as_bytes()) {
        return Some(directory);
    }
    std::fs::create_dir_all(&directory).ok()?;
    crate::control::transaction::atomic_replace(&shim, wanted.as_bytes(), None, false).ok()?;
    Some(directory)
}

fn content(exe: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Written by FastCtx; edits are overwritten. Git Bash rewrites arguments that look like\n\
         # POSIX paths before it starts a native program, which would turn a path on another\n\
         # machine such as /root/app into C:/Program Files/Git/root/app. Through this launcher\n\
         # fastctx receives its arguments exactly as typed.\n\
         MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' exec \"{}\" \"$@\"\n",
        display_path(exe)
    )
}
