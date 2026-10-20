//! Which bash runs a job, and the environment it starts with.
//!
//! The shell is the machine's own bash: `/bin/bash` on Unix, on macOS the
//! one Homebrew installed when there is one, and Git for Windows' bash on
//! Windows (`world-run.md`). It runs as a login shell: it gets the
//! environment a person logging in gets, then reads the profile as their
//! shell would, so a command sees the machine as a person at it does,
//! whichever process started the node. On macOS, where the person's login
//! shell is zsh, the environment is what zsh sets up (`login_shell`).

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::user_environment;
use fastctx_types::{MachineName, TownName};

/// What the shell is told besides the user's environment.
pub struct Context<'a> {
    pub machine: &'a MachineName,
    /// Its resident or page identity, passed to commands the script starts.
    pub owner: &'a fastctx_types::run::JobOwner,
    pub town: Option<&'a TownName>,
    /// The directory of the fastctx binary, put first on the PATH so that
    /// `fastctx view` and the other commands are this same version.
    pub fastctx_dir: &'a Path,
    /// Set when the World lives somewhere other than the default, so that
    /// commands in the shell reach the same World.
    pub fastctx_home: Option<&'a OsStr>,
    /// The job the script is, so that a command in it can tell itself apart
    /// from the other things running, as `fastctx w` does.
    pub job: Option<&'a fastctx_types::JobId>,
    /// Working files retained for this conversation on this machine.
    pub scratch: Option<&'a Path>,
    /// Internal files with the job's confirmed-end lifetime, including `on` state.
    pub job_temp: Option<&'a Path>,
    /// The World's write commands refuse in this shell (`world-write.md`).
    pub writes_off: bool,
    /// The state an `on` brought from the sending script, which the
    /// prelude restores before the script runs.
    pub on_state: Option<&'a Path>,
}

/// A shell invocation, with its complete environment already resolved.
pub struct Prepared {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
}

/// The shell that runs `script` (a file) as the user's login shell would,
/// with its complete environment.
pub fn prepare(script: &Path, context: &Context<'_>) -> io::Result<Prepared> {
    let program = bash()?;
    let script = script_argument(script)?;
    let base = user_environment()?;
    #[cfg(target_os = "macos")]
    if let Some(login) = super::login_shell::environment(&base) {
        return Ok(Prepared { program, args: vec![script], environment: environment_from(login, context) });
    }
    Ok(Prepared { program, args: vec!["--login".into(), script], environment: environment_from(base, context) })
}

/// Adds the World's identity and executable directory to a supplied login
/// environment. Resolving that environment belongs to the caller.
pub fn environment_from(mut env: Vec<(OsString, OsString)>, context: &Context<'_>) -> Vec<(OsString, OsString)> {
    set(&mut env, "FASTCTX_MACHINE", context.machine.as_str());
    match context.owner {
        fastctx_types::run::JobOwner::Resident(address) => {
            set(&mut env, "FASTCTX_RESIDENT", address.tag());
            unset(&mut env, "FASTCTX_PAGE");
        }
        fastctx_types::run::JobOwner::Page { page } => {
            unset(&mut env, "FASTCTX_RESIDENT");
            set(&mut env, "FASTCTX_PAGE", serde_json::to_string(page).expect("page identity serializes"));
        }
    }
    match context.town {
        Some(town) => set(&mut env, "FASTCTX_TOWN", town.as_str()),
        None => unset(&mut env, "FASTCTX_TOWN"),
    }
    if let Some(home) = context.fastctx_home {
        set(&mut env, "FASTCTX_HOME", home);
    }
    match context.job {
        Some(job) => set(&mut env, "FASTCTX_JOB", job.as_str()),
        None => unset(&mut env, "FASTCTX_JOB"),
    }
    // Programs that can draw images write them in one of these terminal
    // protocols; `run` turns them into images (`world-view.md`).
    set(&mut env, "AGENT_GRAPHICS", "kitty,iterm2");
    // Python writes to a pipe in the system's code page, cp1252 on the
    // user's Windows machines, and fails on the first Chinese character.
    // UTF-8 mode is Python's own default from 3.15 (PEP 686).
    set(&mut env, "PYTHONUTF8", "1");
    prepend_path(&mut env, context.fastctx_dir);
    // Bash reads the prelude after the login profile, which may rewrite
    // PATH; the prelude puts this directory first again, in bash's form.
    set(&mut env, "FASTCTX_BIN", shell_path(context.fastctx_dir));
    set(&mut env, "BASH_ENV", forward_slashes(&prelude_path(context.fastctx_dir)));
    match context.scratch {
        Some(scratch) => set(&mut env, "FASTCTX_SCRATCH", forward_slashes(scratch)),
        None => unset(&mut env, "FASTCTX_SCRATCH"),
    }
    match context.job_temp {
        Some(directory) => set(&mut env, "FASTCTX_JOB_TEMP", forward_slashes(directory)),
        None => unset(&mut env, "FASTCTX_JOB_TEMP"),
    }
    match context.on_state {
        Some(state) => set(&mut env, "FASTCTX_ON_STATE", forward_slashes(state)),
        None => unset(&mut env, "FASTCTX_ON_STATE"),
    }
    if context.writes_off {
        set(&mut env, "FASTCTX_WRITES", "off");
    } else {
        unset(&mut env, "FASTCTX_WRITES");
    }
    if cfg!(windows) {
        // What Git Bash itself sets: the tools of its environment come first
        // on the PATH. Which environment an installation has depends on its
        // version, so it is read from the installation.
        match bash().ok().as_deref().and_then(msystem) {
            Some(msystem) => set(&mut env, "MSYSTEM", msystem),
            None => unset(&mut env, "MSYSTEM"),
        }
    }
    env
}

/// The World's prelude, which bash reads through `BASH_ENV` (`world-run.md`).
pub const PRELUDE: &str = include_str!("prelude.bash");
const STATE: &str = include_str!("state.bash");

/// Where the prelude for the World's programs in `fastctx_dir` lives.
pub fn prelude_path(fastctx_dir: &Path) -> PathBuf {
    fastctx_dir.join("prelude.bash")
}

/// Writes the prelude beside the World's programs unless it is there as this
/// version has it.
pub fn ensure_prelude(fastctx_dir: &Path) -> io::Result<()> {
    for (path, text) in [(prelude_path(fastctx_dir), PRELUDE), (fastctx_dir.join("state.bash"), STATE)] {
        if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
            fastctx_platform::publish(&path, text.as_bytes(), fastctx_platform::Permissions::Private)?;
        }
    }
    Ok(())
}

/// A path as bash's own PATH holds it: unchanged on Unix; on Windows in
/// MSYS's form, a drive as a top directory (`C:/x` is `/c/x`), since a colon
/// separates PATH entries. A path without a drive is left as it is.
pub fn shell_path(path: &Path) -> OsString {
    if !cfg!(windows) {
        return path.as_os_str().to_owned();
    }
    let text = forward_slashes(path).to_string_lossy().into_owned();
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let drive = char::from(bytes[0]).to_ascii_lowercase();
        return format!("/{drive}{}", &text[2..]).into();
    }
    text.into()
}

/// A path with forward slashes on Windows, which bash and Windows both read.
fn forward_slashes(path: &Path) -> OsString {
    if cfg!(windows) { path.to_string_lossy().replace('\\', "/").into() } else { path.as_os_str().to_owned() }
}

/// The MSYS environment a Git for Windows installation was built for, by the
/// directory it has beside `usr`: the one holding `git.exe`, else the only
/// one there is.
fn msystem(bash: &Path) -> Option<&'static str> {
    let root = bash.parent()?.parent()?.parent()?;
    let environments = [("ucrt64", "UCRT64"), ("mingw64", "MINGW64"), ("clangarm64", "CLANGARM64")];
    let present: Vec<_> = environments.iter().filter(|(dir, _)| root.join(dir).is_dir()).collect();
    present
        .iter()
        .find(|(dir, _)| root.join(dir).join("bin").join("git.exe").is_file())
        .or(present.first())
        .map(|(_, name)| *name)
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

/// The bash every `run` starts: `/bin/bash`. macOS keeps bash 3.2 there, of
/// 2007; one Homebrew installed, current, comes first.
#[cfg(unix)]
pub fn bash() -> io::Result<PathBuf> {
    let homebrew: &[&str] =
        if cfg!(target_os = "macos") { &["/opt/homebrew/bin/bash", "/usr/local/bin/bash"] } else { &[] };
    homebrew
        .iter()
        .chain(&["/bin/bash"])
        .map(PathBuf::from)
        .find(|bash| bash.is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "/bin/bash does not exist"))
}

#[cfg(unix)]
fn script_argument(script: &Path) -> io::Result<OsString> {
    Ok(script.as_os_str().to_owned())
}

/// Git for Windows' bash: where its installer recorded it, or beside a
/// `git.exe` on the user's PATH, as a portable Git is laid out.
#[cfg(windows)]
pub fn bash() -> io::Result<PathBuf> {
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
    let variable = |wanted: &str| env.iter().find(|(name, _)| same_name(name, wanted)).map(|(_, value)| value);
    if let Some(path) = variable("PATH") {
        for directory in std::env::split_paths(path) {
            if directory.join("git.exe").is_file()
                && let Some(root) = directory.parent()
                && in_root(root).is_file()
            {
                return Ok(in_root(root));
            }
        }
    }
    // Scoop puts only a shim on PATH; Git itself lives under its apps, for
    // the user or for the machine.
    let scoops = [
        variable("SCOOP").map(PathBuf::from),
        variable("USERPROFILE").map(|home| Path::new(home).join("scoop")),
        variable("SCOOP_GLOBAL").map(PathBuf::from),
        variable("ProgramData").map(|data| Path::new(data).join("scoop")),
    ];
    for scoop in scoops.into_iter().flatten() {
        let bash = in_root(&scoop.join("apps").join("git").join("current"));
        if bash.is_file() {
            return Ok(bash);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Git for Windows was not found: run needs its bash; install it from https://git-scm.com",
    ))
}

/// The script's path as bash reads it: forward slashes, which MSYS takes
/// as they are. MSYS splits a native program's command line by its own
/// rules, stripping single quotes and expanding braces and wildcards outside
/// double quotes; the command line quotes such an argument
/// (`fastctx_platform`'s Windows command line), and a Windows path can hold
/// neither a double quote nor, here, a backslash.
#[cfg(windows)]
fn script_argument(script: &Path) -> io::Result<OsString> {
    let text = script.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not valid Unicode", script.display()))
    })?;
    Ok(text.replace('\\', "/").into())
}
