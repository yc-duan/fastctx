//! Which bash runs a job, and the environment it starts with.
//!
//! Until the World's own interpreter replaces it (`world-run.md`), the shell
//! is the machine's bash on Unix and Git for Windows' bash on Windows. It
//! runs as a login shell: it gets the environment a person logging in
//! gets, then reads the profile as their shell would, so a command sees
//! the machine as a person at it does, whichever process started the node.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::user_environment;
use fastctx_types::{ResidentId, TownName};

/// What the shell is told besides the user's environment.
pub struct Context<'a> {
    pub resident: &'a ResidentId,
    pub town: Option<&'a TownName>,
    /// The directory of the fastctx binary, put first on the PATH so that
    /// `fastctx view` and the other commands are this same version.
    pub fastctx_dir: &'a Path,
    /// Set when the World lives somewhere other than the default, so that
    /// commands in the shell reach the same World.
    pub fastctx_home: Option<&'a OsStr>,
}

/// The program and arguments that run `script` (a file) in a login shell.
pub fn command(script: &Path) -> io::Result<(PathBuf, Vec<OsString>)> {
    let bash = bash()?;
    Ok((bash, vec!["--login".into(), script_argument(script)?]))
}

/// The environment the shell starts with.
pub fn environment(context: &Context<'_>) -> io::Result<Vec<(OsString, OsString)>> {
    let mut env = user_environment()?;
    set(&mut env, "FASTCTX_RESIDENT", context.resident.as_str());
    match context.town {
        Some(town) => set(&mut env, "FASTCTX_TOWN", town.as_str()),
        None => unset(&mut env, "FASTCTX_TOWN"),
    }
    if let Some(home) = context.fastctx_home {
        set(&mut env, "FASTCTX_HOME", home);
    }
    // Programs that can draw images write them in one of these terminal
    // protocols; `run` turns them into images (`world-view.md`).
    set(&mut env, "AGENT_GRAPHICS", "kitty,iterm2");
    prepend_path(&mut env, context.fastctx_dir);
    if cfg!(windows) {
        // What Git Bash itself sets: the MinGW tools come first on the PATH.
        set(&mut env, "MSYSTEM", "MINGW64");
    }
    Ok(env)
}

fn same_name(a: &OsStr, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

fn set(env: &mut Vec<(OsString, OsString)>, name: &str, value: impl AsRef<OsStr>) {
    unset(env, name);
    env.push((name.into(), value.as_ref().to_owned()));
}

fn unset(env: &mut Vec<(OsString, OsString)>, name: &str) {
    env.retain(|(existing, _)| !same_name(existing, name));
}

fn prepend_path(env: &mut [(OsString, OsString)], directory: &Path) {
    let separator = if cfg!(windows) { ";" } else { ":" };
    if let Some((_, path)) = env.iter_mut().find(|(name, _)| same_name(name, "PATH")) {
        let mut joined = directory.as_os_str().to_owned();
        joined.push(separator);
        joined.push(&*path);
        *path = joined;
    }
}

#[cfg(unix)]
fn bash() -> io::Result<PathBuf> {
    let bash = PathBuf::from("/bin/bash");
    if bash.is_file() { Ok(bash) } else { Err(io::Error::new(io::ErrorKind::NotFound, "/bin/bash does not exist")) }
}

#[cfg(unix)]
fn script_argument(script: &Path) -> io::Result<OsString> {
    Ok(script.as_os_str().to_owned())
}

/// Git for Windows' bash: where its installer recorded it, or beside a
/// `git.exe` on the user's PATH, as a portable Git is laid out.
#[cfg(windows)]
fn bash() -> io::Result<PathBuf> {
    use fastctx_platform::registry::{Hive, string_value};

    let in_root = |root: &Path| root.join("usr").join("bin").join("bash.exe");
    for hive in [Hive::LocalMachine, Hive::CurrentUser] {
        if let Some(root) = string_value(hive, r"SOFTWARE\GitForWindows", "InstallPath")? {
            let bash = in_root(Path::new(&root));
            if bash.is_file() {
                return Ok(bash);
            }
        }
    }
    let env = user_environment()?;
    if let Some((_, path)) = env.iter().find(|(name, _)| same_name(name, "PATH")) {
        for directory in std::env::split_paths(path) {
            if directory.join("git.exe").is_file()
                && let Some(root) = directory.parent()
                && in_root(root).is_file()
            {
                return Ok(in_root(root));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git for Windows was not found: run needs its bash until the World's own shell replaces it; \
         install it from https://git-scm.com",
    ))
}

/// The script's path as bash reads it: forward slashes, which MSYS takes
/// as they are.
#[cfg(windows)]
fn script_argument(script: &Path) -> io::Result<OsString> {
    let text = script.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not valid Unicode", script.display()))
    })?;
    Ok(text.replace('\\', "/").into())
}
