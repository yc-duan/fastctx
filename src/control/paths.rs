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
    /// A previously applied integration receipt.
    Receipt,
}

/// Origin of the effective DeepSeek Harness home directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DshHomeSource {
    /// An explicit `--dsh-home` command-line override.
    Flag,
    /// The live `DSH_HOME` process environment.
    Environment,
    /// The conventional `<home>/.dsh` fallback.
    Default,
    /// A previously applied integration receipt.
    Receipt,
}

impl DshHomeSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Environment => "env",
            Self::Default => "default",
            Self::Receipt => "receipt",
        }
    }
}

impl CodexHomeSource {
    /// Stable user-facing source label used by Status and Doctor.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Environment => "env",
            Self::Default => "default",
            Self::Receipt => "receipt",
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
    /// DeepSeek Harness machine-level home.
    pub dsh_dir: PathBuf,
    /// Source that selected the DSH home.
    pub dsh_home_source: DshHomeSource,
    /// DeepSeek Harness machine patch file.
    pub dsh_patch: PathBuf,
    /// DeepSeek Harness guidance file.
    pub dsh_agents: PathBuf,
}

impl ControlPaths {
    /// Builds control paths from the current process home environment.
    pub fn discover() -> Result<Self, String> {
        Self::discover_with_codex_home(None)
    }

    /// Builds control paths with an optional command-line Codex profile override.
    pub fn discover_with_codex_home(codex_home: Option<PathBuf>) -> Result<Self, String> {
        Self::discover_with_hosts(codex_home, None)
    }

    /// Builds paths with independent Codex and DeepSeek Harness overrides.
    pub fn discover_with_hosts(
        codex_home: Option<PathBuf>,
        dsh_home: Option<PathBuf>,
    ) -> Result<Self, String> {
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
            Some(path) if !path.is_absolute() => {
                return Err("--codex-home requires an absolute path.".to_string());
            }
            Some(path) => (path, CodexHomeSource::Flag),
            None => match env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
                Some(path) => (PathBuf::from(path), CodexHomeSource::Environment),
                None => (home.join(".codex"), CodexHomeSource::Default),
            },
        };
        let paths = Self::for_home_and_codex_home_and_dsh_home(home, codex_dir, source, dsh_home)?;
        if !paths.dsh_dir.is_absolute() {
            return Err(format!(
                "DeepSeek Harness home from {} must be an absolute path.",
                paths.dsh_home_source.as_str()
            ));
        }
        Ok(paths)
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
        let mut paths = Self::for_home_and_codex_home_and_dsh_home(
            &home,
            codex_dir,
            source,
            Some(home.join(".dsh")),
        )
        .expect("default DSH home is always non-empty");
        paths.dsh_home_source = DshHomeSource::Default;
        paths
    }

    /// Builds paths for supplied host directories. `None` discovers DSH from `DSH_HOME`.
    pub fn for_home_and_codex_home_and_dsh_home(
        home: impl Into<PathBuf>,
        codex_dir: impl Into<PathBuf>,
        source: CodexHomeSource,
        dsh_home: Option<PathBuf>,
    ) -> Result<Self, String> {
        let home = home.into();
        let codex_dir = codex_dir.into();
        let fastctx_dir = home.join(".fastctx");
        let fastctx_bin_dir = fastctx_dir.join("bin");
        let (dsh_dir, dsh_home_source) = match dsh_home {
            Some(path) if path.as_os_str().is_empty() => {
                return Err("--dsh-home requires a non-empty path.".to_string());
            }
            Some(path) => (path, DshHomeSource::Flag),
            None => match env::var_os("DSH_HOME").filter(|value| !value.is_empty()) {
                Some(path) => (PathBuf::from(path), DshHomeSource::Environment),
                None => (home.join(".dsh"), DshHomeSource::Default),
            },
        };
        Ok(Self {
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
            dsh_patch: dsh_dir.join("cordis.patch.yml"),
            dsh_agents: dsh_dir.join("AGENTS.md"),
            dsh_dir,
            dsh_home_source,
        })
    }

    /// Rebinds connected hosts to the absolute homes preserved by their Apply receipts.
    pub fn with_recorded_host_homes(
        &self,
        codex_home: Option<PathBuf>,
        dsh_home: Option<PathBuf>,
    ) -> Result<Self, String> {
        let (codex_dir, codex_source) = match codex_home {
            Some(path) if !path.is_absolute() => {
                return Err(
                    "The saved Codex home must be an absolute path. Reapply that host and retry."
                        .to_string(),
                );
            }
            Some(path) => (path, CodexHomeSource::Receipt),
            None => (self.codex_dir.clone(), self.codex_home_source),
        };
        let (dsh_dir, dsh_source) = match dsh_home {
            Some(path) if !path.is_absolute() => {
                return Err("The saved DeepSeek Harness home must be an absolute path. Reapply that host and retry.".to_string());
            }
            Some(path) => (path, DshHomeSource::Receipt),
            None => (self.dsh_dir.clone(), self.dsh_home_source),
        };
        let mut paths = Self::for_home_and_codex_home_and_dsh_home(
            &self.home,
            codex_dir,
            codex_source,
            Some(dsh_dir),
        )?;
        paths.dsh_home_source = dsh_source;
        Ok(paths)
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
    use super::{CodexHomeSource, ControlPaths, DshHomeSource};

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

    #[test]
    fn explicit_dsh_home_has_flag_source_and_host_paths() {
        let home = std::path::PathBuf::from("example-home");
        let codex = home.join(".codex");
        let dsh = std::path::PathBuf::from("dsh-profile");
        let paths = ControlPaths::for_home_and_codex_home_and_dsh_home(
            &home,
            codex,
            CodexHomeSource::Default,
            Some(dsh.clone()),
        )
        .unwrap();

        assert_eq!(paths.dsh_dir, dsh);
        assert_eq!(paths.dsh_patch, dsh.join("cordis.patch.yml"));
        assert_eq!(paths.dsh_agents, dsh.join("AGENTS.md"));
        assert_eq!(paths.dsh_home_source, DshHomeSource::Flag);
        assert_eq!(paths.fastctx_dir, home.join(".fastctx"));
    }

    #[test]
    fn explicit_empty_dsh_home_is_rejected() {
        let error = ControlPaths::for_home_and_codex_home_and_dsh_home(
            "example-home",
            "codex-profile",
            CodexHomeSource::Default,
            Some(std::path::PathBuf::new()),
        )
        .unwrap_err();

        assert_eq!(error, "--dsh-home requires a non-empty path.");
    }

    #[test]
    fn recorded_host_homes_replace_only_connected_hosts() {
        let home = if cfg!(windows) {
            std::path::PathBuf::from(r"C:\profile")
        } else {
            std::path::PathBuf::from("/profile")
        };
        let paths = ControlPaths::for_home(&home);
        let dsh = home.join("custom-dsh");
        let rebound = paths
            .with_recorded_host_homes(None, Some(dsh.clone()))
            .unwrap();

        assert_eq!(rebound.codex_dir, paths.codex_dir);
        assert_eq!(rebound.codex_home_source, CodexHomeSource::Default);
        assert_eq!(rebound.dsh_dir, dsh);
        assert_eq!(rebound.dsh_home_source, DshHomeSource::Receipt);
    }

    #[test]
    fn relative_recorded_host_home_is_rejected() {
        let paths = ControlPaths::for_home(if cfg!(windows) {
            r"C:\profile"
        } else {
            "/profile"
        });
        let error = paths
            .with_recorded_host_homes(None, Some("relative-dsh".into()))
            .unwrap_err();

        assert!(error.contains("saved DeepSeek Harness home must be an absolute path"));
    }
}
