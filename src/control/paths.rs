//! Stable user-directory paths used by the control terminal.

use std::env;
use std::path::PathBuf;

/// Origin of the effective Codex profile directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexHomeSource {
    /// An explicit `--codex-home` command-line override.
    Flag,
    /// The live `CODEX_HOME` process environment.
    Environment,
    /// The conventional `<home>/.codex` fallback.
    Default,
}

impl CodexHomeSource {
    /// Stable user-facing source label used by Status and Doctor.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Environment => "env",
            Self::Default => "default",
        }
    }
}

/// All paths used by Apply, Unapply, and Status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlPaths {
    /// User home directory.
    pub home: PathBuf,
    /// FastCtx configuration directory.
    pub fastctx_dir: PathBuf,
    /// FastCtx configuration file.
    pub fastctx_config: PathBuf,
    /// Persistent background-job registry and complete output-log directory.
    pub jobs_dir: PathBuf,
    /// Self-installed binary directory.
    pub fastctx_bin_dir: PathBuf,
    /// Stable binary path always referenced by Codex.
    pub installed_binary: PathBuf,
    /// Codex configuration directory.
    pub codex_dir: PathBuf,
    /// Source that selected the Codex configuration directory.
    pub codex_home_source: CodexHomeSource,
    /// Primary Codex configuration file.
    pub codex_config: PathBuf,
    /// Global Codex AGENTS.md file.
    pub codex_agents: PathBuf,
}

impl ControlPaths {
    /// Builds control paths from the current process home environment.
    pub fn discover() -> Result<Self, String> {
        Self::discover_with_codex_home(None)
    }

    /// Builds control paths with an optional command-line Codex profile override.
    pub fn discover_with_codex_home(codex_home: Option<PathBuf>) -> Result<Self, String> {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .or_else(|| env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
            .map(PathBuf::from)
            .ok_or_else(|| {
                "Cannot determine the user home directory. Set HOME or USERPROFILE and retry."
                    .to_string()
            })?;
        let (codex_dir, source) = match codex_home {
            Some(path) if path.as_os_str().is_empty() => {
                return Err("--codex-home requires a non-empty path.".to_string());
            }
            Some(path) => (path, CodexHomeSource::Flag),
            None => match env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
                Some(path) => (PathBuf::from(path), CodexHomeSource::Environment),
                None => (home.join(".codex"), CodexHomeSource::Default),
            },
        };
        Ok(Self::for_home_and_codex_home(home, codex_dir, source))
    }

    /// Builds paths for a supplied home directory for isolated installs and contract tests.
    pub fn for_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let codex_dir = home.join(".codex");
        Self::for_home_and_codex_home(home, codex_dir, CodexHomeSource::Default)
    }

    /// Builds paths for supplied home and Codex profile directories.
    pub fn for_home_and_codex_home(
        home: impl Into<PathBuf>,
        codex_dir: impl Into<PathBuf>,
        source: CodexHomeSource,
    ) -> Self {
        let home = home.into();
        let codex_dir = codex_dir.into();
        let fastctx_dir = home.join(".fastctx");
        let fastctx_bin_dir = fastctx_dir.join("bin");
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
            codex_home_source: source,
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

#[cfg(test)]
mod tests {
    use super::{CodexHomeSource, ControlPaths};

    #[test]
    fn explicit_codex_profile_never_moves_fastctx_state() {
        let home = std::path::PathBuf::from("example-home");
        let profile = std::path::PathBuf::from("codex-work-profile");
        let paths = ControlPaths::for_home_and_codex_home(&home, &profile, CodexHomeSource::Flag);

        assert_eq!(paths.codex_dir, profile);
        assert_eq!(paths.codex_config, profile.join("config.toml"));
        assert_eq!(paths.fastctx_dir, home.join(".fastctx"));
        assert_eq!(paths.codex_home_source, CodexHomeSource::Flag);
    }
}
