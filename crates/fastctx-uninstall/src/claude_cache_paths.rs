//! Path derivation in Claude Code 2.1.296. Login history is not a cache.

use anyhow::Context as _;
use fastctx_install::uninstall::Installation;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub(crate) struct Roots {
    pub cache: PathBuf,
    pub temporary: PathBuf,
}

impl Roots {
    pub fn here(installation: &Installation) -> anyhow::Result<Self> {
        // Background Claude sessions inherit the user's logon environment,
        // not the environment of the GUI or the uninstall helper.
        let environment = fastctx_platform::user_environment()?;
        let platform = if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        };
        #[cfg(unix)]
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 0;
        let roots = Self::from_environment(platform, &installation.home, uid, &environment)?;
        anyhow::ensure!(
            roots.cache.is_absolute() && roots.temporary.is_absolute(),
            "Claude working-directory cache and temporary roots must be absolute"
        );
        let physical = |path: PathBuf| -> anyhow::Result<PathBuf> {
            match std::fs::canonicalize(&path) {
                Ok(path) => Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
                Err(error) => Err(error.into()),
            }
        };
        Ok(Self { cache: physical(roots.cache)?, temporary: physical(roots.temporary)? })
    }

    pub(crate) fn from_environment(
        platform: Platform,
        home: &Path,
        uid: u32,
        environment: &[(OsString, OsString)],
    ) -> anyhow::Result<Self> {
        let get = |name: &str| -> Option<&OsStr> {
            environment
                .iter()
                .find(
                    |(key, _)| if platform == Platform::Windows { key.eq_ignore_ascii_case(name) } else { key == name },
                )
                .map(|(_, value)| value.as_os_str())
                .filter(|value| !value.is_empty())
        };
        let home = get(if platform == Platform::Windows { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_path_buf());
        let cache = if platform == Platform::Windows {
            get("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Local"))
                .join("claude-cli-nodejs/Cache")
        } else if platform == Platform::Mac {
            home.join("Library/Caches/claude-cli-nodejs")
        } else {
            get("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache")).join("claude-cli-nodejs")
        };
        let temporary = if let Some(path) = get("CLAUDE_CODE_TMPDIR") {
            PathBuf::from(path)
        } else if platform == Platform::Windows {
            if let Some(path) = get("TEMP").or_else(|| get("TMP")) {
                PathBuf::from(path)
            } else {
                PathBuf::from(
                    get("SystemRoot")
                        .or_else(|| get("windir"))
                        .context("Claude's Windows temporary directory has no SystemRoot or windir")?,
                )
                .join("temp")
            }
        } else if platform == Platform::Mac {
            PathBuf::from("/tmp")
        } else {
            get("TMPDIR")
                .or_else(|| get("TMP"))
                .or_else(|| get("TEMP"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
        };
        let name = if platform == Platform::Windows { "claude".into() } else { format!("claude-{uid}") };
        let temporary = temporary.join(name);
        Ok(Self { cache, temporary })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Linux,
    Mac,
}
