//! Update-check and update-transaction data shared by the TUI and updater helper.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Official npm registry used for version authority and the `official` source.
pub(crate) const OFFICIAL_NPM_REGISTRY: &str = "https://registry.npmjs.org/";
/// Built-in China mirror whose publication visibility is maintained by the release runbook.
pub(crate) const NPMMIRROR_REGISTRY: &str = "https://registry.npmmirror.com/";

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

/// Why an npm target version is trusted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum NpmVersionAuthority {
    /// At least one official channel (GitHub or registry.npmjs.org) supplied the target.
    Official,
    /// Both official channels were unavailable, so a configured or built-in mirror supplied it.
    MirrorFallback,
}

/// One registry's visible state for an exact npm update target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct NpmRegistryProbe {
    /// Human-readable origin such as npm config, official npm, or npmmirror.
    pub(crate) source_name: String,
    /// Normalized registry URL passed to npm.
    pub(crate) registry: String,
    /// Whether the registry answered a valid latest-version query.
    pub(crate) reachable: bool,
    /// Stable dist-tags.latest value, when reachable.
    pub(crate) latest_version: Option<String>,
    /// Whether the exact main package target exists on this registry.
    pub(crate) main_package_ready: bool,
    /// Whether the exact local-platform package target exists on this registry.
    pub(crate) platform_package_ready: bool,
    /// One-line transient or structural diagnostic retained for the update page.
    pub(crate) error: Option<String>,
    /// Failure severity for `error`, when present.
    pub(crate) error_kind: Option<CheckFailureKind>,
}

impl NpmRegistryProbe {
    /// Whether this source can safely install the exact target without a half-install.
    pub(crate) const fn is_ready(&self) -> bool {
        self.reachable && self.main_package_ready && self.platform_package_ready
    }
}

/// Complete source-selection evidence cached and shown by the npm update page.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct NpmDiscovery {
    /// Persisted source policy used for this check.
    pub(crate) source_policy: String,
    /// Effective npm-config registry, if the local query returned a usable URL.
    pub(crate) configured_registry: Option<String>,
    /// Exact authoritative version evaluated by every source.
    pub(crate) target_version: String,
    /// Version-authority path used for the target.
    pub(crate) authority: NpmVersionAuthority,
    /// Whether GitHub supplied a valid stable version this round.
    pub(crate) github_version: Option<String>,
    /// Whether the official npm registry supplied a valid stable version this round.
    pub(crate) official_version: Option<String>,
    /// Local-platform package name whose exact version was preflighted.
    pub(crate) platform_package: String,
    /// Deterministically ordered source table.
    pub(crate) probes: Vec<NpmRegistryProbe>,
    /// Selected ready registry URL, absent when the target is still propagating.
    pub(crate) selected_registry: Option<String>,
    /// Selected source's human-readable origin.
    pub(crate) selected_source: Option<String>,
    /// Human-readable deterministic selection rationale.
    pub(crate) selection_reason: String,
}

/// A fully pinned update the helper can apply without resolving mutable input again.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "channel", rename_all = "kebab-case")]
pub(crate) enum UpdatePlan {
    /// Install one exact version through the npm CLI.
    Npm {
        /// Trusted launcher provenance.
        provenance: NpmProvenance,
        /// Exact version selected by the authority decision.
        target_version: String,
        /// Exact normalized registry selected after the two-package preflight.
        registry: String,
        /// Human-readable source name shown on the confirmation screen.
        source_name: String,
        /// Complete evidence shown by the dedicated update page.
        discovery: Box<NpmDiscovery>,
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
            Self::Npm {
                provenance,
                source_name,
                registry,
                ..
            } => format!("npm · {} · {source_name} ({registry})", provenance.package),
            Self::GithubRelease { .. } => "GitHub Release".to_string(),
        }
    }

    /// Returns npm source-selection evidence when this is an npm update.
    pub(crate) fn npm_discovery(&self) -> Option<&NpmDiscovery> {
        match self {
            Self::Npm { discovery, .. } => Some(discovery.as_ref()),
            Self::GithubRelease { .. } => None,
        }
    }
}

/// Result of the silent startup check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StartupUpdate {
    /// The current version is latest, the channel is unsupported, or checking is disabled.
    None,
    /// The current npm version is authoritative and the source table is available for inspection.
    NpmCurrent {
        /// Complete source-selection evidence.
        discovery: Box<NpmDiscovery>,
    },
    /// A newer version can be installed immediately.
    Available(Box<UpdatePlan>),
    /// GitHub is newer but npm has not exposed the matching package version yet.
    NpmPending {
        /// Exact authoritative version that no selected source can yet install safely.
        target_version: String,
        /// Complete source-selection evidence.
        discovery: Box<NpmDiscovery>,
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
