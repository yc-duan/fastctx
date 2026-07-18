//! Update-check and update-transaction data shared by the TUI and updater helper.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// User-facing severity for an update discovery failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CheckFailureKind {
    /// Connectivity, rate limiting, captive portals, and other retryable conditions stay quiet.
    Transient,
    /// A successfully reached publication surface returned content that violates its contract.
    Structural,
}

/// Classified update discovery failure.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CheckFailure {
    /// Presentation severity.
    pub(crate) kind: CheckFailureKind,
    /// Stable diagnostic retained for Status.
    pub(crate) message: String,
}

/// npm invocation style that launched the current FastCtx process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NpmMode {
    /// A persistent globally installed package.
    Global,
    /// An ephemeral `npm exec` or `npx` package.
    Exec,
}

/// Provenance supplied by the npm launcher.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct NpmProvenance {
    /// Root package that owns the launcher.
    pub(crate) package: String,
    /// Whether the package is global or ephemeral.
    pub(crate) mode: NpmMode,
    /// Node.js executable used to run npm without shell interpolation.
    pub(crate) node: PathBuf,
    /// npm CLI JavaScript entry point.
    pub(crate) npm_cli: PathBuf,
    /// Launcher entry point to execute after a global update.
    pub(crate) launcher: PathBuf,
    /// npm launcher process that keeps the invoking terminal foreground-owned.
    pub(crate) launcher_pid: u32,
    /// Private marker the launcher watches until the updated TUI session ends.
    pub(crate) handoff_file: PathBuf,
}

/// A fully pinned update the helper can apply without resolving mutable input again.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "channel", rename_all = "kebab-case")]
pub(crate) enum UpdatePlan {
    /// Install one exact version through the npm CLI.
    Npm {
        /// Trusted launcher provenance.
        provenance: NpmProvenance,
        /// Exact public-registry version selected by the check.
        target_version: String,
    },
    /// Install one exact asset from the latest stable GitHub Release.
    GithubRelease {
        /// Exact semantic version from the release tag.
        target_version: String,
        /// Platform archive filename.
        archive_name: String,
        /// Immutable platform archive URL derived from the validated tag.
        archive_url: String,
        /// Immutable aggregate checksum URL derived from the validated tag.
        checksums_url: String,
    },
}

impl UpdatePlan {
    /// Exact version the plan installs.
    pub(crate) fn target_version(&self) -> &str {
        match self {
            Self::Npm { target_version, .. } | Self::GithubRelease { target_version, .. } => {
                target_version
            }
        }
    }

    /// Short source label suitable for the update screen.
    pub(crate) fn source_label(&self) -> String {
        match self {
            Self::Npm { provenance, .. } => format!("npm · {}", provenance.package),
            Self::GithubRelease { .. } => "GitHub Release".to_string(),
        }
    }
}

/// Result of the silent startup check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupUpdate {
    /// The current version is latest, the channel is unsupported, or checking is disabled.
    None,
    /// A newer version can be installed immediately.
    Available(UpdatePlan),
    /// GitHub is newer but npm has not exposed the matching package version yet.
    NpmPending {
        /// Stable release version already visible on GitHub.
        release_version: String,
        /// Latest version currently visible through a fresh npm cache.
        registry_version: String,
    },
    /// The check failed; startup may continue without mutating the installation.
    Failed(CheckFailure),
    /// An attempted update failed and the previous installation was reopened.
    InstallFailed(String),
}

/// Durable handoff from the exiting TUI to the copied updater helper.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct UpdateRequest {
    /// Request format version.
    pub(crate) schema_version: u32,
    /// Version of the process that created the request.
    pub(crate) current_version: String,
    /// Selected immutable update.
    pub(crate) plan: UpdatePlan,
    /// Executable currently providing the TUI.
    pub(crate) target_executable: PathBuf,
    /// Copied helper that survives replacement of the installation.
    pub(crate) helper_executable: PathBuf,
    /// File the restarted process creates after finalization succeeds.
    pub(crate) health_file: PathBuf,
}
