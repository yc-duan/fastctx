//! Stable user-directory paths used by the control terminal.

use std::env;
use std::path::PathBuf;

/// All paths used by Apply, Unapply, and Status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlPaths {
    /// User home directory.
    pub home: PathBuf,
    /// FastCtx configuration directory.
    pub fastctx_dir: PathBuf,
    /// FastCtx configuration file.
    pub fastctx_config: PathBuf,
    /// Persistent background-job registry and spool directory.
    pub jobs_dir: PathBuf,
    /// Self-installed binary directory.
    pub fastctx_bin_dir: PathBuf,
    /// Stable binary path always referenced by Codex.
    pub installed_binary: PathBuf,
    /// Codex configuration directory.
    pub codex_dir: PathBuf,
    /// Primary Codex configuration file.
    pub codex_config: PathBuf,
    /// Global Codex AGENTS.md file.
    pub codex_agents: PathBuf,
}

impl ControlPaths {
    /// Builds control paths from the current process home environment.
    pub fn discover() -> Result<Self, String> {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .or_else(|| env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
            .map(PathBuf::from)
            .ok_or_else(|| {
                "Cannot determine the user home directory. Set HOME or USERPROFILE and retry."
                    .to_string()
            })?;
        Ok(Self::for_home(home))
    }

    /// Builds paths for a supplied home directory for isolated installs and contract tests.
    pub fn for_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let fastctx_dir = home.join(".fastctx");
        let fastctx_bin_dir = fastctx_dir.join("bin");
        let codex_dir = home.join(".codex");
        Self {
            fastctx_config: fastctx_dir.join("config.toml"),
            jobs_dir: fastctx_dir.join("jobs"),
            installed_binary: fastctx_bin_dir.join(installed_binary_name()),
            codex_config: codex_dir.join("config.toml"),
            codex_agents: codex_dir.join("AGENTS.md"),
            home,
            fastctx_dir,
            fastctx_bin_dir,
            codex_dir,
        }
    }
}

#[cfg(windows)]
fn installed_binary_name() -> &'static str {
    "fastctx.exe"
}

#[cfg(not(windows))]
fn installed_binary_name() -> &'static str {
    "fastctx"
}
