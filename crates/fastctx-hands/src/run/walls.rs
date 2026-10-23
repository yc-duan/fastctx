//! The walls a sentinel's command runs behind.
//!
//! A sentinel watches for something and must never change anything or reach
//! anyone (`world-senses.md`). Its command runs as the machine's own user, so
//! it reads what that user can read, but the operating system holds it to
//! that: the filesystem is read-only, there is no network, and the World's
//! own directory, with its sockets and keys, is hidden, so a command talked
//! into trying can neither act as the machine nor carry its secrets away.
//!
//! Linux confines with bubblewrap (`bwrap`), macOS with `sandbox-exec`.
//! Windows has no walls: a sentinel's command runs in Git Bash as the
//! machine's user, as the user chose, held only by the sentinel's own rules.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// The program and the arguments that put a command behind the walls, up to
/// where the shell and its own arguments go, or none where a command runs as
/// it is. `world` is hidden, and each of `keep` inside it is put back
/// read-only: the script to run, and where it starts.
pub fn walls(world: &Path, keep: &[&Path]) -> io::Result<Option<(PathBuf, Vec<OsString>)>> {
    platform::walls(world, keep)
}

/// Why commands cannot be put behind the walls on this machine, if they cannot.
pub fn missing() -> Option<String> {
    platform::missing()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    const MISSING: &str =
        "bubblewrap (bwrap) is not installed on this machine, so a sentinel cannot run commands here";

    fn bwrap() -> Option<PathBuf> {
        ["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"].into_iter().map(PathBuf::from).find(|path| path.exists())
    }

    pub fn missing() -> Option<String> {
        bwrap().is_none().then(|| MISSING.to_string())
    }

    /// The whole tree read-only with a fresh `/dev` and scratch, the World's
    /// directory an empty one, and every namespace unshared, so there is no
    /// network and the walls go down with all inside. The machine's own
    /// `/proc` stays, so its processes can be looked at, and so do its GPUs.
    pub fn walls(world: &Path, keep: &[&Path]) -> io::Result<Option<(PathBuf, Vec<OsString>)>> {
        let bwrap = bwrap().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, MISSING))?;
        let mut args: Vec<OsString> =
            ["--ro-bind", "/", "/", "--dev", "/dev", "--tmpfs", "/tmp", "--tmpfs", "/run", "--tmpfs", "/var/tmp"]
                .into_iter()
                .map(OsString::from)
                .collect();
        for gpu in std::fs::read_dir("/dev").into_iter().flatten().flatten() {
            if gpu.file_name().to_string_lossy().starts_with("nvidia") {
                args.extend(["--dev-bind".into(), gpu.path().into_os_string(), gpu.path().into_os_string()]);
            }
        }
        args.extend(["--tmpfs".into(), world.as_os_str().to_owned()]);
        for path in keep {
            args.extend(["--ro-bind".into(), path.as_os_str().to_owned(), path.as_os_str().to_owned()]);
        }
        args.extend(["--unshare-all", "--die-with-parent", "--new-session", "--cap-drop", "ALL", "--"].map(OsString::from));
        Ok(Some((bwrap, args)))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const SANDBOX: &str = "/usr/bin/sandbox-exec";

    pub fn missing() -> Option<String> {
        (!Path::new(SANDBOX).exists())
            .then(|| "sandbox-exec is not available on this machine, so a sentinel cannot run commands here".into())
    }

    fn quoted(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// No network, no writes but to the terminal devices and scratch, and the
    /// World's directory unreadable but for what is kept; the later rule wins.
    pub fn walls(world: &Path, keep: &[&Path]) -> io::Result<Option<(PathBuf, Vec<OsString>)>> {
        let mut profile = format!(
            "(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n\
             (allow file-write-data (regex #\"^/dev/null$\") (regex #\"^/dev/tty\") (regex #\"^/dev/std\"))\n\
             (allow file-write* (subpath \"/tmp\") (subpath \"/private/tmp\") (subpath \"/var/tmp\"))\n\
             (deny file-read* (subpath \"{}\"))\n",
            quoted(world)
        );
        for path in keep {
            profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", quoted(path)));
        }
        Ok(Some((PathBuf::from(SANDBOX), vec!["-p".into(), profile.into()])))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;

    pub fn missing() -> Option<String> {
        None
    }

    pub fn walls(_world: &Path, _keep: &[&Path]) -> io::Result<Option<(PathBuf, Vec<OsString>)>> {
        Ok(None)
    }
}
