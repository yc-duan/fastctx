//! A read-only, offline look at one machine, for a sentinel.
//!
//! A sentinel watches for something and must never change anything or reach
//! anyone (`world-senses.md`). Its one command here runs as the machine's own
//! user, so it can read whatever that user can read, but the operating system
//! holds it to that: the whole filesystem is mounted read-only, there is no
//! network, and the World's own socket is replaced by a dead file so a command
//! that is talked into trying cannot act as the machine. This is not the job
//! system: it is a short command with a deadline, whose every byte of output
//! is capped and whose process tree is taken down the moment the deadline
//! passes.
//!
//! Linux confines with bubblewrap (`bwrap`); macOS with `sandbox-exec`. On
//! Windows a sentinel cannot run commands at all — Git for Windows' bash does
//! not start under a restricted token — so a sentinel there is given only the
//! World's own read-only views (file, directory, search, job output), not this.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::ffi::OsString;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::Write as _;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::mpsc;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use fastctx_platform::Child;

/// What to look at, and the walls around it.
pub struct Confine<'a> {
    /// The command, as the sentinel wrote it. It is fed to bash on standard
    /// input, so no temporary file is left where a tmpfs could hide it.
    pub command: &'a str,
    /// Where bash starts. It must be a readable directory on this machine.
    pub cwd: &'a Path,
    /// The directory of the World's binary, put first on the PATH so that the
    /// command's `fastctx view` is this same version.
    pub fastctx_dir: &'a Path,
    /// The user's home directory, so the command sees the machine as the user.
    pub home: &'a Path,
    /// Set when this World lives somewhere other than the default.
    pub fastctx_home: Option<&'a OsStr>,
    /// Paths replaced by `/dev/null` inside the sandbox: the node's own
    /// socket, and any supervisor sockets, so a command cannot speak to them.
    pub mask: &'a [PathBuf],
    /// How long the command may run before its process tree is taken down.
    pub deadline: Duration,
    /// The most bytes of output kept; the rest is drained and noted as elided.
    pub limit: usize,
}

/// What a look saw.
pub struct Reading {
    /// The command's merged standard output and error, capped at the limit.
    pub text: String,
    /// Its exit code, or None when it was signalled or stopped at the deadline.
    pub exit_code: Option<i32>,
    /// Whether output was longer than the limit and the rest was dropped.
    pub truncated: bool,
}

/// Runs `command` on this machine under a read-only, offline sandbox.
pub fn look(confine: &Confine<'_>) -> io::Result<Reading> {
    platform::look(confine)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// The environment the command starts with: a predictable PATH with the
/// World's binary first, the user's home, UTF-8 for Python, and writes off so
/// the World's write commands refuse. No profile is read, so nothing reorders
/// the PATH. Locale and timezone pass through so text renders as it would.
fn environment(confine: &Confine<'_>) -> Vec<(OsString, OsString)> {
    let mut path = OsString::from(confine.fastctx_dir);
    for dir in ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        path.push(":");
        path.push(dir);
    }
    let mut env = vec![
        (OsString::from("PATH"), path),
        (OsString::from("HOME"), confine.home.as_os_str().to_owned()),
        (OsString::from("PYTHONUTF8"), OsString::from("1")),
        (OsString::from("FASTCTX_WRITES"), OsString::from("off")),
        (OsString::from("TMPDIR"), OsString::from("/tmp")),
    ];
    if let Some(home) = confine.fastctx_home {
        env.push((OsString::from("FASTCTX_HOME"), home.to_owned()));
    }
    for name in ["LANG", "LC_ALL", "LC_CTYPE", "TERM", "TZ"] {
        if let Some(value) = std::env::var_os(name) {
            env.push((OsString::from(name), value));
        }
    }
    env
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// The script bash reads: the command with its error output merged into its
/// standard output, as a terminal shows it. Anything the sandbox tool itself
/// writes stays on its own standard error, where a setup failure is read.
fn script(command: &str) -> Vec<u8> {
    format!("exec 2>&1\n{command}\n").into_bytes()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// Starts `program`, feeds it the script, and returns what it wrote with how
/// it ended, taking its process tree down if the deadline passes first.
fn confined(
    program: &Path,
    args: &[OsString],
    env: &[(OsString, OsString)],
    cwd: &Path,
    command: &str,
    deadline: Duration,
    limit: usize,
) -> io::Result<(Vec<u8>, Vec<u8>, Option<i32>, bool, bool)> {
    let refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let mut child = Child::spawn(program, &refs, cwd, env)?;
    // The script is short and fits the pipe buffer, so the write never blocks.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&script(command))?;
    }
    let out = child.stdout.take();
    let err = child.stderr.take();
    let reading = std::thread::spawn(move || out.map(|r| drain(r, limit)).unwrap_or_default());
    let erring = std::thread::spawn(move || err.map(|r| drain(r, 8192).0).unwrap_or_default());
    let child = Arc::new(child);
    let waiter = {
        let child = child.clone();
        let (done, waited) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(child.wait());
        });
        waited
    };
    let (status, timed_out) = match waiter.recv_timeout(deadline) {
        Ok(status) => (Some(status?), false),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            child.kill()?;
            (None, true)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => (None, false),
    };
    let (stdout, truncated) = reading.join().unwrap_or_default();
    let stderr = erring.join().unwrap_or_default();
    let code = status.and_then(|status| status.code());
    Ok((stdout, stderr, code, truncated, timed_out))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// Reads to the end, keeping at most `limit` bytes from the start and counting
/// the rest. Draining to the end keeps the command from blocking on a full pipe.
fn drain(mut reader: impl io::Read, limit: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < limit {
                    let room = limit - kept.len();
                    kept.extend_from_slice(&buffer[..n.min(room)]);
                    if n > room {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (kept, truncated)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// Turns captured bytes into text with a note when something was cut or stopped.
fn reading(stdout: Vec<u8>, code: Option<i32>, truncated: bool, timed_out: bool, deadline: Duration) -> Reading {
    let mut text = String::from_utf8_lossy(&stdout).into_owned();
    if truncated {
        text.push_str("\n[output was longer than the limit; the rest was dropped]");
    }
    if timed_out {
        text.push_str(&format!("\n[the command was stopped after {} seconds]", deadline.as_secs()));
    }
    Reading { text, exit_code: code, truncated }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
/// Where a program lives: common directories first, then the PATH.
fn program_path(name: &str) -> Option<PathBuf> {
    for base in ["/usr/bin/", "/bin/", "/usr/local/bin/"] {
        let candidate = Path::new(base).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|dir| dir.join(name)).find(|candidate| candidate.exists())
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    /// The bubblewrap walls: the whole tree read-only, a fresh `/dev`, `/proc`
    /// and writable scratch, the World's sockets masked, and every namespace
    /// unshared — so there is no network and nothing of this process carries in.
    pub fn look(confine: &Confine<'_>) -> io::Result<Reading> {
        let bwrap = program_path("bwrap").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "bubblewrap (bwrap) is not installed on this machine; a sentinel cannot run commands here",
            )
        })?;
        let bash = super::super::shell::bash()?;
        let mut args: Vec<OsString> = Vec::new();
        for flag in [
            "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--tmpfs", "/run",
            "--tmpfs", "/var/tmp",
        ] {
            args.push(flag.into());
        }
        // After the read-only tree, replace each World socket with a dead file:
        // a connect to it then fails instead of reaching the node.
        for path in confine.mask {
            args.push("--ro-bind".into());
            args.push("/dev/null".into());
            args.push(path.as_os_str().to_owned());
        }
        for flag in ["--unshare-all", "--die-with-parent", "--new-session", "--cap-drop", "ALL", "--"] {
            args.push(flag.into());
        }
        args.push(bash.into_os_string());
        for flag in ["--noprofile", "--norc", "-s"] {
            args.push(flag.into());
        }
        let env = environment(confine);
        let (stdout, stderr, code, truncated, timed_out) =
            confined(&bwrap, &args, &env, confine.cwd, confine.command, confine.deadline, confine.limit)?;
        // bwrap writes its own setup failures with a `bwrap:` prefix and exits
        // before bash runs; that is a sandbox error, not the command's output.
        if code != Some(0) && stderr.starts_with(b"bwrap:") {
            let message = String::from_utf8_lossy(&stderr).trim().to_owned();
            return Err(io::Error::other(format!("the sentinel sandbox could not start: {message}")));
        }
        Ok(reading(stdout, code, truncated, timed_out, confine.deadline))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    /// A sandbox-exec profile denying all network and all writes but to the
    /// null and terminal devices. Best effort: the fleet this serves is Linux.
    const PROFILE: &str = r#"(version 1)
(allow default)
(deny network*)
(deny file-write*)
(allow file-write-data (regex #"^/dev/null$") (regex #"^/dev/tty") (regex #"^/dev/std"))
(allow file-write* (subpath "/tmp") (subpath "/private/tmp") (subpath "/var/tmp"))"#;

    pub fn look(confine: &Confine<'_>) -> io::Result<Reading> {
        let sandbox = Path::new("/usr/bin/sandbox-exec");
        if !sandbox.exists() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sandbox-exec is not available on this machine; a sentinel cannot run commands here",
            ));
        }
        let bash = super::super::shell::bash()?;
        let args: Vec<OsString> = vec![
            "-p".into(),
            PROFILE.into(),
            bash.into_os_string(),
            "--noprofile".into(),
            "--norc".into(),
            "-s".into(),
        ];
        let env = environment(confine);
        let (stdout, stderr, code, truncated, timed_out) =
            confined(sandbox, &args, &env, confine.cwd, confine.command, confine.deadline, confine.limit)?;
        if code != Some(0) && stderr.starts_with(b"sandbox-exec:") {
            let message = String::from_utf8_lossy(&stderr).trim().to_owned();
            return Err(io::Error::other(format!("the sentinel sandbox could not start: {message}")));
        }
        Ok(reading(stdout, code, truncated, timed_out, confine.deadline))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;

    pub fn look(_confine: &Confine<'_>) -> io::Result<Reading> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a sentinel cannot run commands on this machine; here it can only read files, \
             list directories, search, and read a job's output",
        ))
    }
}
