//! The environment the machine prepares for this user, independent of
//! whichever process started the caller.

use std::ffi::OsString;
use std::io;

/// The user's environment as the system defines it for someone logging in.
///
/// On Windows that is the system and user variables from the registry,
/// merged as a fresh logon merges them, read anew on every call so a
/// variable the user just changed is seen without restarting anything;
/// for SYSTEM, its profile and data folders are the home Fastctx gives it
/// (`crate::system_home`).
///
/// On Unix it is what a login session holds before the login shell reads
/// its profile: the account's name, home and shell, then what PAM's
/// `pam_env` reads from `/etc/environment` and `/etc/default/locale`, the
/// login PATH from `/etc/login.defs` when those files give none, and the
/// per-user runtime directory and session bus when they exist. A login
/// shell started with it reads the profile itself.
pub fn user_environment() -> io::Result<Vec<(OsString, OsString)>> {
    let mut env = imp::user_environment()?;
    for (name, value) in crate::system_home::variables()? {
        env.retain(|(held, _)| !held.eq_ignore_ascii_case(&name));
        env.push((name, value));
    }
    Ok(env)
}

/// The environment for a fastctx process the World starts on its own (a
/// node, a supervisor): the user's, plus what of this process's says which
/// World it is, so the new process belongs to the same one: `FASTCTX_HOME`,
/// and `FASTCTX_UPDATE_RELEASES`, where a drill's World looks for releases.
/// A development build also carries which agents' homes it was given: one
/// runs in a sandbox of its own, and a node a command of it starts must
/// never fall back to the user's own Codex or Claude Code login
/// (`crate::home::RELEASED`).
pub fn world_process_environment() -> io::Result<Vec<(OsString, OsString)>> {
    let mut env = user_environment()?;
    let sandboxed: &[&str] = if crate::home::RELEASED { &[] } else { &["CODEX_HOME", "CLAUDE_CONFIG_DIR"] };
    for carried in ["FASTCTX_HOME", "FASTCTX_UPDATE_RELEASES"].iter().chain(sandboxed) {
        let carried = *carried;
        if let Some(value) = std::env::var_os(carried) {
            env.retain(|(name, _)| name != carried);
            env.push((carried.into(), value));
        }
    }
    Ok(env)
}

/// Whether this process belongs to any job object. Job membership alone
/// does not say whether closing a handle kills it. Never on Unix.
pub fn in_job() -> io::Result<bool> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut result = 0;
        // SAFETY: queries this process; NULL asks about any job.
        if unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut result) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(result != 0)
    }
    #[cfg(unix)]
    Ok(false)
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt as _;

    use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};

    use crate::windows_user::with_current_token;

    pub fn user_environment() -> io::Result<Vec<(OsString, OsString)>> {
        with_current_token(|token| {
            let mut block: *mut std::ffi::c_void = std::ptr::null_mut();
            // SAFETY: the block is freed below; FALSE leaves this process's
            // own variables out.
            if unsafe { CreateEnvironmentBlock(&mut block, token, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut variables = Vec::new();
            let mut cursor = block.cast::<u16>();
            // SAFETY: the block is a sequence of NUL-terminated UTF-16
            // strings ended by an empty one.
            unsafe {
                loop {
                    let length = (0..).take_while(|&i| *cursor.add(i) != 0).count();
                    if length == 0 {
                        break;
                    }
                    let entry = std::slice::from_raw_parts(cursor, length);
                    // A name may start with `=` (per-drive directories); the
                    // separator is the first `=` after the first character.
                    if let Some(split) = entry.iter().skip(1).position(|&unit| unit == u16::from(b'=')) {
                        let split = split + 1;
                        variables
                            .push((OsString::from_wide(&entry[..split]), OsString::from_wide(&entry[split + 1..])));
                    }
                    cursor = cursor.add(length + 1);
                }
                DestroyEnvironmentBlock(block);
            }
            Ok(variables)
        })
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};

    pub fn user_environment() -> io::Result<Vec<(OsString, OsString)>> {
        let account = crate::unix_account::current()?;
        let mut env = Environment::default();
        env.set("USER", &account.name);
        env.set("LOGNAME", &account.name);
        env.set("HOME", &account.home);
        env.set("SHELL", &account.shell);
        for file in ["/etc/environment", "/etc/default/locale"] {
            match std::fs::read(file) {
                Ok(text) => apply_pam_env(&text, &mut env),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io::Error::new(error.kind(), format!("{file}: {error}"))),
            }
        }
        if env.get("PATH").is_none() {
            let path = login_defs_path(account.uid == 0)?;
            env.set("PATH", OsStr::from_bytes(&path));
        }
        let runtime = format!("/run/user/{}", account.uid);
        if std::fs::metadata(&runtime).is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == account.uid) {
            let bus = format!("{runtime}/bus");
            if std::fs::metadata(&bus).is_ok_and(|metadata| metadata.file_type().is_socket()) {
                env.set("DBUS_SESSION_BUS_ADDRESS", OsStr::new(&format!("unix:path={bus}")));
            }
            env.set("XDG_RUNTIME_DIR", OsStr::new(&runtime));
        }
        Ok(env.0)
    }

    /// Ordered variables, later settings replacing earlier ones.
    #[derive(Default)]
    struct Environment(Vec<(OsString, OsString)>);

    impl Environment {
        fn set(&mut self, name: &str, value: impl AsRef<OsStr>) {
            self.unset(name);
            self.0.push((name.into(), value.as_ref().to_owned()));
        }

        fn unset(&mut self, name: &str) {
            self.0.retain(|(existing, _)| existing != name);
        }

        fn get(&self, name: &str) -> Option<&OsStr> {
            self.0.iter().find(|(existing, _)| existing == name).map(|(_, value)| value.as_os_str())
        }
    }

    /// Applies a file in `pam_env`'s simple format, as Linux-PAM 1.5 reads
    /// `/etc/environment`: `#` starts a comment wherever it appears, a
    /// trailing backslash joins the next line, an `export ` prefix is
    /// ignored, names are letters, digits and underscores, one surrounding
    /// quote is stripped from each end of the value, nothing is expanded,
    /// and a name without `=` removes the variable. Malformed lines are
    /// skipped, as `pam_env` skips them.
    fn apply_pam_env(text: &[u8], env: &mut Environment) {
        let mut logical = Vec::new();
        for line in text.split(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(continued) = line.strip_suffix(b"\\") {
                logical.extend_from_slice(continued);
                continue;
            }
            logical.extend_from_slice(line);
            apply_line(&std::mem::take(&mut logical), env);
        }
        if !logical.is_empty() {
            apply_line(&logical, env);
        }
    }

    fn apply_line(line: &[u8], env: &mut Environment) {
        let line = match line.iter().position(|&b| b == b'#') {
            Some(comment) => &line[..comment],
            None => line,
        };
        let line = line.trim_ascii();
        let line = line.strip_prefix(b"export ").map_or(line, <[u8]>::trim_ascii_start);
        if line.is_empty() {
            return;
        }
        let (name, value) = match line.iter().position(|&b| b == b'=') {
            Some(split) => (&line[..split], Some(&line[split + 1..])),
            None => (line, None),
        };
        if name.is_empty() || !name.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_') {
            return;
        }
        let name = std::str::from_utf8(name).expect("checked ASCII");
        match value {
            None => env.unset(name),
            Some(value) => {
                let value = value.strip_prefix(b"\"").or_else(|| value.strip_prefix(b"'")).unwrap_or(value);
                let value = value.strip_suffix(b"\"").or_else(|| value.strip_suffix(b"'")).unwrap_or(value);
                env.set(name, OsStr::from_bytes(value));
            }
        }
    }

    /// The PATH login(1) gives: `ENV_SUPATH` or `ENV_PATH` from
    /// `/etc/login.defs`, else login's own built-in defaults.
    fn login_defs_path(root: bool) -> io::Result<Vec<u8>> {
        let key: &[u8] = if root { b"ENV_SUPATH" } else { b"ENV_PATH" };
        let text = match std::fs::read("/etc/login.defs") {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        for line in text.split(|&b| b == b'\n') {
            let mut words = line.split(u8::is_ascii_whitespace).filter(|word| !word.is_empty());
            if words.next() == Some(key)
                && let Some(value) = words.next()
            {
                return Ok(value.strip_prefix(b"PATH=").unwrap_or(value).to_vec());
            }
        }
        Ok(if root { b"/sbin:/bin:/usr/sbin:/usr/bin".to_vec() } else { b"/bin:/usr/bin".to_vec() })
    }
}
