//! The environment a macOS user's own login shell sets up. macOS makes zsh
//! the login shell, and the PATH and other variables a person relies on,
//! Homebrew's among them, are set in its startup files, which bash does not
//! read. A job starts from what that shell exports; bash then reads no
//! profile of its own, whose path_helper would put the system's directories
//! back before the user's.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a login shell may take to read its startup files: one that
/// waits for input or for the network is not waited for.
const SETTING_UP: Duration = Duration::from_secs(10);

/// Where the shell is told to write what it exports. A file, not a pipe: a
/// program its startup files leave running in the background would hold a
/// pipe open after the shell is gone.
const OUTPUT: &str = "FASTCTX_LOGIN_ENVIRONMENT";

/// What the login shell `base` names exports once it has read its startup
/// files; `None` when it is bash, which reads its own as a login shell, or
/// when it does not answer in time.
pub(super) fn environment(base: &[(OsString, OsString)]) -> Option<Vec<(OsString, OsString)>> {
    let variable = |wanted: &str| base.iter().find(|(name, _)| name == wanted).map(|(_, value)| value.as_os_str());
    let shell = Path::new(variable("SHELL")?);
    if !shell.is_absolute() || shell.file_name() == Some(OsStr::new("bash")) || !shell.is_file() {
        return None;
    }
    let output = tempfile::NamedTempFile::new().ok()?;
    let mut child = Command::new(shell)
        // Written so that zsh, bash, ksh, fish and tcsh all read it alike.
        .args(["-l", "-c", &format!("exec /usr/bin/env -0 > \"${OUTPUT}\"")])
        .env_clear()
        .envs(base.iter().map(|(name, value)| (name, value)))
        .env(OUTPUT, output.path())
        .current_dir(variable("HOME")?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + SETTING_UP;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let exported = parse(std::fs::read(output.path()).ok()?);
    let has = |wanted: &str| exported.iter().any(|(name, _)| name == wanted);
    (has("PATH") && has("HOME")).then_some(exported)
}

/// `env -0`'s entries, without what describes the shell that printed them
/// rather than the user's environment.
fn parse(bytes: Vec<u8>) -> Vec<(OsString, OsString)> {
    bytes
        .split(|&byte| byte == 0)
        .filter_map(|entry| {
            let split = entry.iter().position(|&byte| byte == b'=').filter(|&at| at > 0)?;
            let name = OsString::from_vec(entry[..split].to_vec());
            let shell_own = ["PWD", "OLDPWD", "SHLVL", "_", OUTPUT].iter().any(|own| name == *own);
            (!shell_own).then(|| (name, OsString::from_vec(entry[split + 1..].to_vec())))
        })
        .collect()
}
