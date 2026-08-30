//! Stable format and atomic I/O for `~/.fastctx/config.toml`.

use crate::control::agents::InsertedSeparator;
use crate::control::i18n::ALL_LANGUAGES;
use crate::control::paths::ControlPaths;
use crate::control::targets::AgentTarget;
use crate::control::transaction;
use crate::search_parallelism::{self, SearchParallelism};
use crate::server_manifest::EnabledTools;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

const CURRENT_SCHEMA_VERSION: u32 = 2;
const LEGACY_SCHEMA_VERSION: u32 = 0;
const SINGLE_TARGET_SCHEMA_VERSION: u32 = 1;
/// Generation of the recommended per-tool budget defaults.
///
/// Bump this whenever the tier limits or the per-tool defaults move. Startup then clears stored
/// per-tool overrides once and shows the migration notice, because a share is relative: the same
/// "50%" resolves to a different absolute budget after the global budget changes, so carrying an
/// override across the change would silently rescale a setting nobody revisited. Configurations
/// written before this field existed read as generation 0 and migrate on the next launch.
pub(crate) const TOOL_BUDGET_EPOCH: u32 = 2;
/// Default current-user disk allowance for retained background-job records.
pub const DEFAULT_JOB_STORAGE_LIMIT_MIB: u64 = 1_024;
/// Default current-user number of simultaneously running background jobs.
pub const DEFAULT_MAX_RUNNING_JOBS: u64 = 128;
/// Default number of background-job records returned by one `job_list` call.
pub const DEFAULT_JOB_LIST_LIMIT: u64 = 20;
/// Largest configurable page size accepted by `job_list`.
pub const MAX_JOB_LIST_LIMIT: u64 = 100;
/// Default replace input and output safety limit in MiB.
pub const DEFAULT_REPLACE_FILE_LIMIT_MIB: i64 = 256;
/// Smallest replace safety limit accepted by the control plane.
pub const MIN_REPLACE_FILE_LIMIT_MIB: i64 = 64;
/// Largest replace safety limit offered by the control plane.
pub const MAX_REPLACE_FILE_LIMIT_MIB: i64 = 4_096;

/// Effective current-user job limits plus whether persisted values required fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobLimitStatus {
    /// Effective disk limit in MiB.
    pub job_storage_limit_mib: u64,
    /// Effective cross-session running-job limit.
    pub max_running_jobs: u64,
    /// Effective default page size for `job_list`.
    pub job_list_limit: u64,
    /// Whether the stored disk limit was present but invalid.
    pub storage_limit_fell_back: bool,
    /// Whether the stored running limit was present but invalid.
    pub running_limit_fell_back: bool,
    /// Whether the stored `job_list` page size was present but invalid.
    pub list_limit_fell_back: bool,
}

/// Effective machine-level update settings plus persisted-value diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpdateSettingsStatus {
    /// Whether TUI startup checks are enabled.
    pub auto_check: bool,
    /// Effective npm update-source policy.
    pub source: UpdateSource,
    /// Whether a present persisted source value was invalid and fell back to `auto`.
    pub source_fell_back: bool,
}

/// Effective search parallelism plus a diagnosable invalid persisted limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchParallelismStatus {
    /// Engine-visible upper bound derived from `available_parallelism` and the hard cap.
    pub available: usize,
    /// Raw explicit user value, or `None` for automatic parallelism.
    pub configured: Option<i64>,
    /// Effective `P`; absent when the explicit value is outside `1..=available`.
    pub effective: Option<usize>,
}

/// Current-user grep/glob CPU settings, read when the shared control center starts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Maximum CPU lanes including the request-local base lane; omission keeps automatic mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cpu_cores: Option<i64>,
}

impl SearchSettings {
    fn is_default(&self) -> bool {
        self.max_cpu_cores.is_none()
    }
}

/// Current-user replace memory-safety settings, reloaded for every request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ReplaceSettings {
    /// Maximum size of both an input file and its replacement result, in MiB.
    pub max_file_size_mib: i64,
}

impl ReplaceSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Resolves a persisted limit without silently clamping an invalid user choice.
    pub(crate) fn resolved_file_limit_mib(self) -> Result<u64, String> {
        if !(MIN_REPLACE_FILE_LIMIT_MIB..=MAX_REPLACE_FILE_LIMIT_MIB)
            .contains(&self.max_file_size_mib)
        {
            return Err(format!(
                "replace.max_file_size_mib must be a whole number from {MIN_REPLACE_FILE_LIMIT_MIB}..={MAX_REPLACE_FILE_LIMIT_MIB} MiB"
            ));
        }
        Ok(self.max_file_size_mib as u64)
    }
}

impl Default for ReplaceSettings {
    fn default() -> Self {
        Self {
            max_file_size_mib: DEFAULT_REPLACE_FILE_LIMIT_MIB,
        }
    }
}

/// npm download-source policy for source-aware updates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSource {
    /// Probe the effective npm registry, official npm, and npmmirror in deterministic order.
    #[default]
    Auto,
    /// Strictly use the registry returned by `npm config get registry`.
    NpmConfig,
    /// Strictly use the official npm registry.
    Official,
    /// Strictly use registry.npmmirror.com.
    Npmmirror,
}

impl UpdateSource {
    /// Stable configuration value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::NpmConfig => "npm-config",
            Self::Official => "official",
            Self::Npmmirror => "npmmirror",
        }
    }

    /// Parses a persisted source value, returning `None` for an unsupported value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "npm-config" => Some(Self::NpmConfig),
            "official" => Some(Self::Official),
            "npmmirror" => Some(Self::Npmmirror),
            _ => None,
        }
    }

    /// Selects the previous source cyclically.
    pub const fn previous(self) -> Self {
        match self {
            Self::Auto => Self::Npmmirror,
            Self::NpmConfig => Self::Auto,
            Self::Official => Self::NpmConfig,
            Self::Npmmirror => Self::Official,
        }
    }

    /// Selects the next source cyclically.
    pub const fn next(self) -> Self {
        match self {
            Self::Auto => Self::NpmConfig,
            Self::NpmConfig => Self::Official,
            Self::Official => Self::Npmmirror,
            Self::Npmmirror => Self::Auto,
        }
    }
}

impl<'de> Deserialize<'de> for UpdateSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value.as_str().and_then(Self::parse).unwrap_or(Self::Auto))
    }
}

/// Machine-level update preferences saved independently from Apply.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Whether TUI startup should automatically check for updates.
    pub auto_check: bool,
    /// npm download-source policy.
    pub source: UpdateSource,
}

/// Protection applied when the visible Codex provider has no remote compaction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct OutputGuardSettings {
    /// Whether third-party providers are constrained to the Guarded output policy.
    pub enabled: bool,
}

impl OutputGuardSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for OutputGuardSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            source: UpdateSource::Auto,
        }
    }
}

/// Codex host output tier.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// Conservative 20k host limit with an 18k FastCtx budget.
    Compact,
    /// Recommended 60k host limit with a 54k FastCtx budget.
    #[default]
    Standard,
    /// Widest 100k host limit with a 90k FastCtx budget.
    #[serde(alias = "extra-high")]
    #[value(alias = "extra-high")]
    High,
}

impl Tier {
    /// Host token limit written to Codex.
    pub const fn host_limit(self) -> i64 {
        match self {
            Self::Compact => 20_000,
            Self::Standard => 60_000,
            Self::High => 100_000,
        }
    }

    /// Global token budget written to the FastCtx environment, ten percent below the host limit.
    pub const fn fastctx_budget(self) -> usize {
        match self {
            Self::Compact => 18_000,
            Self::Standard => 54_000,
            Self::High => 90_000,
        }
    }

    /// Recommended per-tool shares for this tier, used wherever the user set no explicit share.
    ///
    /// Only `read` scales with the tier: it is the one tool asked to deliver a whole file at once,
    /// so a wider tier exists precisely to widen it. The other four deliver excerpts, listings,
    /// summaries, or output that also survives on disk, and their useful size barely moves between
    /// tiers, so their shares shrink as the global budget grows. Each tool's resolved absolute
    /// budget still rises monotonically from Compact to High; `tier_defaults_grow_with_the_tier`
    /// fails if a future edit breaks that.
    pub const fn default_budgets(self) -> ToolBudgets {
        match self {
            Self::Compact => ToolBudgets {
                read: ToolBudgetLevel::Inherit,
                grep: ToolBudgetLevel::Percent(50),
                glob: ToolBudgetLevel::Percent(25),
                run: ToolBudgetLevel::Percent(50),
                job_output: ToolBudgetLevel::Percent(25),
            },
            Self::Standard => ToolBudgets {
                read: ToolBudgetLevel::Inherit,
                grep: ToolBudgetLevel::Percent(20),
                glob: ToolBudgetLevel::Percent(10),
                run: ToolBudgetLevel::Percent(20),
                job_output: ToolBudgetLevel::Percent(10),
            },
            Self::High => ToolBudgets {
                read: ToolBudgetLevel::Inherit,
                grep: ToolBudgetLevel::Percent(12),
                glob: ToolBudgetLevel::Percent(6),
                run: ToolBudgetLevel::Percent(12),
                job_output: ToolBudgetLevel::Percent(6),
            },
        }
    }

    /// Stable English identifier used by configuration and CLI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Standard => "standard",
            Self::High => "high",
        }
    }

    /// Tier proper name shown by the UI and kept in English in every language.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Standard => "Standard",
            Self::High => "High",
        }
    }

    /// Selects the previous tier cyclically.
    pub const fn previous(self) -> Self {
        match self {
            Self::Compact => Self::High,
            Self::Standard => Self::Compact,
            Self::High => Self::Standard,
        }
    }

    /// Selects the next tier cyclically.
    pub const fn next(self) -> Self {
        match self {
            Self::Compact => Self::Standard,
            Self::Standard => Self::High,
            Self::High => Self::Compact,
        }
    }
}

/// One tool's share of the global budget, as a whole percent.
///
/// A full share is `Inherit` rather than `Percent(100)`: it omits the per-tool environment
/// variable entirely so the server falls back to the global budget, which also keeps budget
/// errors pointing at the global variable instead of an equal per-tool one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolBudgetLevel {
    /// Omit the per-tool environment variable so the server inherits the global value.
    #[default]
    Inherit,
    /// Explicit share between 1 and 99 percent of the global budget.
    Percent(u8),
}

impl ToolBudgetLevel {
    /// Builds a share from a whole percent, normalizing a full share to inheritance.
    ///
    /// Rejects anything outside `1..=100`; a zero share resolves to a budget the server refuses.
    pub const fn from_percent(percent: u8) -> Option<Self> {
        if percent == 0 || percent > 100 {
            None
        } else if percent == 100 {
            Some(Self::Inherit)
        } else {
            Some(Self::Percent(percent))
        }
    }

    /// Share of the global budget as a whole percent, where inheritance is the full budget.
    pub const fn percent(self) -> u8 {
        match self {
            Self::Inherit => 100,
            Self::Percent(percent) => percent,
        }
    }

    /// Returns the concrete budget to write, or `None` for inheritance.
    pub const fn resolve(self, global: usize) -> Option<usize> {
        let percent = match self {
            Self::Inherit => return None,
            Self::Percent(percent) => percent as usize,
        };
        let raw = (global * percent + 50) / 100;
        let rounded = ((raw + 50) / 100) * 100;
        // Rounding to hundreds reaches zero for a small share of a small budget, and the server
        // rejects a zero budget outright, so keep the smallest representable step instead.
        Some(if rounded == 0 { 100 } else { rounded })
    }

    /// Token ceiling this share resolves to, where inheritance is the whole global budget.
    pub const fn ceiling(self, global: usize) -> usize {
        match self.resolve(global) {
            Some(value) => value,
            None => global,
        }
    }

    /// Percentage label shown by the UI and accepted back by the configuration file.
    pub fn label(self) -> String {
        format!("{}%", self.percent())
    }

    /// Parses a configuration-file spelling.
    ///
    /// The four fixed names are what releases before arbitrary percentages wrote, so they stay
    /// readable forever; `"17%"` is accepted too because that is how the UI shows a share and
    /// therefore how someone hand-editing the file will most likely spell it.
    fn from_config_str(value: &str) -> Option<Self> {
        match value.trim() {
            "inherit" => Some(Self::Inherit),
            "percent75" => Some(Self::Percent(75)),
            "percent50" => Some(Self::Percent(50)),
            "percent25" => Some(Self::Percent(25)),
            other => Self::from_percent(other.strip_suffix('%')?.trim().parse().ok()?),
        }
    }
}

impl Serialize for ToolBudgetLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Shares an older release can still parse keep their legacy spelling so a downgrade reads
        // the file back instead of failing on an unrecognized shape. Only a share that older
        // releases could never express falls through to the numeric form. (2026-07-25)
        match self {
            Self::Inherit => serializer.serialize_str("inherit"),
            Self::Percent(75) => serializer.serialize_str("percent75"),
            Self::Percent(50) => serializer.serialize_str("percent50"),
            Self::Percent(25) => serializer.serialize_str("percent25"),
            Self::Percent(percent) => serializer.serialize_u8(*percent),
        }
    }
}

impl<'de> Deserialize<'de> for ToolBudgetLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ToolBudgetLevelVisitor)
    }
}

struct ToolBudgetLevelVisitor;

impl serde::de::Visitor<'_> for ToolBudgetLevelVisitor {
    type Value = ToolBudgetLevel;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("\"inherit\" or a whole percent between 1 and 100")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        ToolBudgetLevel::from_config_str(value)
            .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Str(value), &self))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        u8::try_from(value)
            .ok()
            .and_then(ToolBudgetLevel::from_percent)
            .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Unsigned(value), &self))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let unsigned = u64::try_from(value)
            .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(value), &self))?;
        self.visit_u64(unsigned)
    }
}

/// The five long-output tools' shares in effect for one Apply.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ToolBudgets {
    /// Share for read.
    pub read: ToolBudgetLevel,
    /// Share for grep.
    pub grep: ToolBudgetLevel,
    /// Share for glob.
    pub glob: ToolBudgetLevel,
    /// Share for run; effective only when the shell group is enabled.
    pub run: ToolBudgetLevel,
    /// Share for job_output; effective only when the shell group is enabled.
    pub job_output: ToolBudgetLevel,
}

/// Per-tool shares the user set explicitly; every unset entry follows the selected tier.
///
/// Keeping "unset" distinct from an equal explicit share is what lets a tier change re-target the
/// budgets nobody touched while preserving the ones somebody did. Collapsing the two would leave
/// only bad options: either a tier change silently discards explicit shares, or one edit freezes
/// that tool at a share chosen for a different global budget.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ToolBudgetPreferences {
    /// Explicit share for read, or `None` to follow the tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<ToolBudgetLevel>,
    /// Explicit share for grep, or `None` to follow the tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grep: Option<ToolBudgetLevel>,
    /// Explicit share for glob, or `None` to follow the tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<ToolBudgetLevel>,
    /// Explicit share for run, or `None` to follow the tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<ToolBudgetLevel>,
    /// Explicit share for job_output, or `None` to follow the tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_output: Option<ToolBudgetLevel>,
}

impl ToolBudgetPreferences {
    /// Fills every unset entry from the tier's recommended shares.
    pub fn resolve(self, tier: Tier) -> ToolBudgets {
        let defaults = tier.default_budgets();
        ToolBudgets {
            read: self.read.unwrap_or(defaults.read),
            grep: self.grep.unwrap_or(defaults.grep),
            glob: self.glob.unwrap_or(defaults.glob),
            run: self.run.unwrap_or(defaults.run),
            job_output: self.job_output.unwrap_or(defaults.job_output),
        }
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// One optional tool-group toggle in `~/.fastctx/config.toml`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct FeatureToggle {
    /// Whether the next Apply should publish this tool group.
    pub enabled: bool,
}

/// Fastshell publication choice plus current-user background-job limits.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct FastShellSettings {
    /// Whether the next Apply should publish the shell tools.
    pub enabled: bool,
    /// Maximum total size of the persistent job registry before terminal records are reaped.
    #[serde(deserialize_with = "deserialize_job_storage_limit")]
    pub job_storage_limit_mib: u64,
    /// Maximum number of background jobs running across all FastCtx sessions.
    #[serde(deserialize_with = "deserialize_max_running_jobs")]
    pub max_running_jobs: u64,
    /// Default maximum records returned by `job_list`; explicit tool arguments override it once.
    #[serde(deserialize_with = "deserialize_job_list_limit")]
    pub job_list_limit: u64,
}

impl Default for FastShellSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            job_storage_limit_mib: DEFAULT_JOB_STORAGE_LIMIT_MIB,
            max_running_jobs: DEFAULT_MAX_RUNNING_JOBS,
            job_list_limit: DEFAULT_JOB_LIST_LIMIT,
        }
    }
}

fn deserialize_job_storage_limit<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_positive_or_default(deserializer, DEFAULT_JOB_STORAGE_LIMIT_MIB)
}

fn deserialize_max_running_jobs<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_positive_or_default(deserializer, DEFAULT_MAX_RUNNING_JOBS)
}

fn deserialize_job_list_limit<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .filter(|value| (1..=MAX_JOB_LIST_LIMIT).contains(value))
        .unwrap_or(DEFAULT_JOB_LIST_LIMIT))
}

fn deserialize_positive_or_default<'de, D>(deserializer: D, fallback: u64) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .filter(|value| *value > 0)
        .unwrap_or(fallback))
}

impl Default for ToolBudgets {
    fn default() -> Self {
        Tier::default().default_budgets()
    }
}

/// Receipt for one user file managed by Apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedFileRecord {
    /// Absolute path of the managed file.
    pub path: String,
    /// Whether the file existed before Apply; Unapply uses this to decide whether to delete an empty file.
    /// Older receipts missing the field default to false, conservatively preferring an empty file over deleting user data.
    #[serde(default)]
    pub original_existed: bool,
    /// SHA-256 of post-Apply bytes for ownership-sensitive operations.
    /// Status validates managed semantics instead of hashing a shared whole file.
    pub applied_sha256: String,
}

/// Complete receipt for the most recent successful Apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppliedRecord {
    /// UTC time of Apply.
    pub applied_at_utc: String,
    /// FastCtx version that performed Apply.
    pub version: String,
    /// Stable absolute binary path written to Codex.
    pub command: String,
    /// Host tier selected for that Apply.
    pub tier: Tier,
    /// Host token limit written to Codex.
    pub tool_output_token_limit: i64,
    /// Explicit Codex MCP tool timeout written by Apply; absent in pre-2026-07-23 receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_sec: Option<i64>,
    /// Whether the shared host key existed before Apply.
    pub previous_token_limit_present: bool,
    /// Pre-Apply value of the shared host key, present only when the key existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_token_limit: Option<i64>,
    /// Global budget written to the server.
    pub fastctx_token_budget: usize,
    /// Five long-output tools' relative budget choices.
    pub tool_budgets: ToolBudgets,
    /// Whether fastshell was registered by this Apply.
    #[serde(default)]
    pub fastshell_enabled: bool,
    /// Exact 1.0 tool set; older receipts derive it from fastshell_enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<EnabledTools>,
    /// Legacy receipt field accepted so older installations can be re-applied safely.
    #[serde(default, skip_serializing)]
    pub fastedit_enabled: bool,
    /// Whether Apply created the effective Codex profile directory; Unapply removes that owned shell only while it remains empty.
    #[serde(default)]
    pub codex_dir_created: bool,
    /// Ownership receipt for Codex config.
    pub codex_config: ManagedFileRecord,
    /// Ownership receipt for Codex AGENTS.md.
    pub codex_agents: ManagedFileRecord,
    /// Managed-section contract recorded by an explicit Apply; absent in older receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_contract_id: Option<String>,
    /// Leading AGENTS separator inserted by the first Apply, used as reverse-operation ownership evidence only without drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_agents_inserted_separator: Option<InsertedSeparator>,
    /// Content hash of the self-installed binary.
    pub binary_sha256: String,
}

impl AppliedRecord {
    /// Reports whether this receipt owns the Codex files selected by the current profile resolver.
    pub fn targets_codex_profile(&self, paths: &ControlPaths) -> bool {
        paths_refer_to_same_location(Path::new(&self.codex_config.path), &paths.codex_config)
            && paths_refer_to_same_location(Path::new(&self.codex_agents.path), &paths.codex_agents)
    }
}

/// Shared self-installation receipt used by every target connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallationRecord {
    pub version: String,
    pub command: String,
    pub binary_sha256: String,
}

/// Codex-only ownership and host-budget facts retained inside its target receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodexReceipt {
    pub tier: Tier,
    pub tool_output_token_limit: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_sec: Option<i64>,
    pub previous_token_limit_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_token_limit: Option<i64>,
    pub tool_budgets: ToolBudgets,
    #[serde(default)]
    pub codex_dir_created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_contract_id: Option<String>,
}

/// Minimal inverse-edit evidence for one source-preserving JSONC MCP insertion.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct JsoncConfigReceipt {
    /// Smallest property path inserted by the first Apply, such as `mcpServers.fastctx`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserted_path: Vec<String>,
    /// Whether the insertion parent used a trailing comma before FastCtx appended its property.
    #[serde(default)]
    pub parent_had_trailing_comma: bool,
    /// Whether Apply created the root object from an empty existing file.
    #[serde(default)]
    pub root_was_empty: bool,
}

/// Ownership receipt for one agent target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TargetReceipt {
    pub applied_at_utc: String,
    pub version: String,
    pub enabled_tools: EnabledTools,
    pub fastctx_token_budget: usize,
    pub config: ManagedFileRecord,
    pub guidance: ManagedFileRecord,
    pub config_entry_sha256: String,
    pub guidance_managed_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance_inserted_separator: Option<InsertedSeparator>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_directories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsonc_config: Option<JsoncConfigReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<CodexReceipt>,
}

fn paths_refer_to_same_location(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// FastCtx's own configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct FastCtxSettings {
    /// Configuration format version.
    pub schema_version: u32,
    /// Software-version watermark maintained by startup normalization and fresh-install writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_version: Option<String>,
    /// Generation of the per-tool budget defaults this file has been reconciled against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_budget_epoch: Option<u32>,
    /// TUI language; absence means first-run selection is incomplete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Host tier used by the next Apply.
    pub tier: Tier,
    /// Advanced per-tool overrides used by the next Apply; unset entries follow the tier.
    #[serde(skip_serializing_if = "ToolBudgetPreferences::is_default")]
    pub tool_budgets: ToolBudgetPreferences,
    /// Provider-derived output protection; omission keeps the safe enabled default.
    #[serde(skip_serializing_if = "OutputGuardSettings::is_default")]
    pub output_guard: OutputGuardSettings,
    /// Optional fastshell server, disabled by default.
    pub fastshell: FastShellSettings,
    /// Machine-level update preferences, effective immediately when saved.
    pub update: UpdateSettings,
    /// Current-user grep/glob CPU limit, effective after the shared control center restarts.
    #[serde(skip_serializing_if = "SearchSettings::is_default")]
    pub search: SearchSettings,
    /// Current-user replace input and result limit, effective on the next replace request.
    #[serde(skip_serializing_if = "ReplaceSettings::is_default")]
    pub replace: ReplaceSettings,
    /// Legacy config key accepted but omitted from every newly written settings file.
    #[serde(default, skip_serializing)]
    pub fastedit: FeatureToggle,
    /// Shared stable binary installation receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<InstallationRecord>,
    /// Last selected tool set per target, retained across Disconnect.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub target_preferences: BTreeMap<String, EnabledTools>,
    /// Per-target connection and ownership receipts.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub applied_targets: BTreeMap<String, TargetReceipt>,
    /// Schema-v1 Codex receipt accepted and rebuilt in memory for the legacy control implementation.
    #[serde(default, skip_serializing)]
    pub applied: Option<AppliedRecord>,
}

/// Settings prepared for a user-facing control-plane startup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartupSettings {
    /// Loaded settings, with a fresh install's version watermark prepared in memory.
    pub(crate) settings: FastCtxSettings,
    /// Whether this startup migrated a pre-watermark configuration and must notify the user.
    pub(crate) migration_notice: bool,
}

impl Default for FastCtxSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            last_seen_version: None,
            tool_budget_epoch: None,
            language: None,
            tier: Tier::Standard,
            tool_budgets: ToolBudgetPreferences::default(),
            output_guard: OutputGuardSettings::default(),
            fastshell: FastShellSettings::default(),
            update: UpdateSettings::default(),
            search: SearchSettings::default(),
            replace: ReplaceSettings::default(),
            fastedit: FeatureToggle::default(),
            installation: None,
            target_preferences: BTreeMap::new(),
            applied_targets: BTreeMap::new(),
            applied: None,
        }
    }
}

impl FastCtxSettings {
    pub fn target_receipt(&self, target: AgentTarget) -> Option<&TargetReceipt> {
        self.applied_targets.get(target.id())
    }

    pub fn selected_tools(&self, target: AgentTarget) -> EnabledTools {
        self.target_preferences
            .get(target.id())
            .copied()
            .or_else(|| {
                self.target_receipt(target)
                    .map(|receipt| receipt.enabled_tools)
            })
            .unwrap_or_else(EnabledTools::files)
    }

    pub fn set_target_receipt(&mut self, target: AgentTarget, receipt: TargetReceipt) {
        self.target_preferences
            .insert(target.id().to_string(), receipt.enabled_tools);
        self.applied_targets
            .insert(target.id().to_string(), receipt);
    }

    pub fn remove_target_receipt(&mut self, target: AgentTarget) -> Option<TargetReceipt> {
        if target == AgentTarget::Codex {
            self.applied = None;
        }
        self.applied_targets.remove(target.id())
    }

    fn normalize_receipts_after_decode(&mut self, source_schema: u32) -> Result<(), String> {
        for id in self
            .target_preferences
            .keys()
            .chain(self.applied_targets.keys())
        {
            AgentTarget::from_str(id).map_err(|_| {
                format!("FastCtx settings contain unsupported agent target id \"{id}\".")
            })?;
        }
        if source_schema <= SINGLE_TARGET_SCHEMA_VERSION {
            let legacy_tools = if self.fastshell.enabled {
                EnabledTools::all()
            } else {
                EnabledTools::files()
            };
            self.target_preferences
                .entry(AgentTarget::Codex.id().to_string())
                .or_insert(legacy_tools);
            self.sync_codex_receipt_to_v2();
        } else {
            self.rebuild_legacy_codex_view();
        }
        self.schema_version = CURRENT_SCHEMA_VERSION;
        Ok(())
    }

    fn sync_codex_receipt_to_v2(&mut self) {
        let Some(record) = self.applied.clone() else {
            return;
        };
        self.installation = Some(InstallationRecord {
            version: record.version.clone(),
            command: record.command.clone(),
            binary_sha256: record.binary_sha256.clone(),
        });
        let enabled_tools = record.enabled_tools.unwrap_or_else(|| {
            if record.fastshell_enabled {
                EnabledTools::all()
            } else {
                EnabledTools::files()
            }
        });
        self.target_preferences
            .entry(AgentTarget::Codex.id().to_string())
            .or_insert(enabled_tools);
        let previous = self.applied_targets.get(AgentTarget::Codex.id());
        let config_entry_sha256 = previous
            .map(|receipt| receipt.config_entry_sha256.clone())
            .unwrap_or_default();
        let guidance_managed_sha256 = previous
            .map(|receipt| receipt.guidance_managed_sha256.clone())
            .unwrap_or_else(|| record.codex_agents.applied_sha256.clone());
        self.applied_targets.insert(
            AgentTarget::Codex.id().to_string(),
            TargetReceipt {
                applied_at_utc: record.applied_at_utc.clone(),
                version: record.version.clone(),
                enabled_tools,
                fastctx_token_budget: record.fastctx_token_budget,
                config: record.codex_config.clone(),
                guidance: record.codex_agents.clone(),
                config_entry_sha256,
                guidance_managed_sha256,
                guidance_inserted_separator: record.codex_agents_inserted_separator,
                created_directories: if record.codex_dir_created {
                    record
                        .codex_config
                        .path
                        .as_str()
                        .rsplit_once(['/', '\\'])
                        .map(|(directory, _)| vec![directory.to_string()])
                        .unwrap_or_default()
                } else {
                    Vec::new()
                },
                jsonc_config: None,
                codex: Some(CodexReceipt {
                    tier: record.tier,
                    tool_output_token_limit: record.tool_output_token_limit,
                    tool_timeout_sec: record.tool_timeout_sec,
                    previous_token_limit_present: record.previous_token_limit_present,
                    previous_token_limit: record.previous_token_limit,
                    tool_budgets: record.tool_budgets,
                    codex_dir_created: record.codex_dir_created,
                    agents_contract_id: record.agents_contract_id.clone(),
                }),
            },
        );
    }

    fn rebuild_legacy_codex_view(&mut self) {
        let Some(installation) = self.installation.as_ref() else {
            self.applied = None;
            return;
        };
        let Some(receipt) = self.applied_targets.get(AgentTarget::Codex.id()) else {
            self.applied = None;
            return;
        };
        let Some(codex) = receipt.codex.as_ref() else {
            self.applied = None;
            return;
        };
        self.applied = Some(AppliedRecord {
            applied_at_utc: receipt.applied_at_utc.clone(),
            version: receipt.version.clone(),
            command: installation.command.clone(),
            tier: codex.tier,
            tool_output_token_limit: codex.tool_output_token_limit,
            tool_timeout_sec: codex.tool_timeout_sec,
            previous_token_limit_present: codex.previous_token_limit_present,
            previous_token_limit: codex.previous_token_limit,
            fastctx_token_budget: receipt.fastctx_token_budget,
            tool_budgets: codex.tool_budgets,
            fastshell_enabled: receipt.enabled_tools.shell_enabled(),
            enabled_tools: Some(receipt.enabled_tools),
            fastedit_enabled: false,
            codex_dir_created: codex.codex_dir_created,
            codex_config: receipt.config.clone(),
            codex_agents: receipt.guidance.clone(),
            agents_contract_id: codex.agents_contract_id.clone(),
            codex_agents_inserted_separator: receipt.guidance_inserted_separator,
            binary_sha256: installation.binary_sha256.clone(),
        });
    }
}

/// Loads FastCtx configuration, returning defaults when the file does not exist.
pub fn load(paths: &ControlPaths) -> Result<FastCtxSettings, String> {
    load_from(&paths.fastctx_config)
}

/// Normalizes settings for TUI and write-capable CLI startup.
///
/// An existing file without a software-version watermark is migrated atomically. A missing file
/// is not created here; its in-memory defaults are stamped so the first natural save cannot be
/// mistaken for an upgrade on the next launch.
pub(crate) fn load_for_startup(paths: &ControlPaths) -> Result<StartupSettings, String> {
    const MAX_COMMIT_ATTEMPTS: usize = 3;

    for attempt in 0..MAX_COMMIT_ATTEMPTS {
        let original = transaction::read_snapshot(&paths.fastctx_config)?;
        let Some(original) = original else {
            let settings = FastCtxSettings {
                last_seen_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                tool_budget_epoch: Some(TOOL_BUDGET_EPOCH),
                ..FastCtxSettings::default()
            };
            return Ok(StartupSettings {
                settings,
                migration_notice: false,
            });
        };
        let source = std::str::from_utf8(&original).map_err(|error| {
            format!(
                "Cannot read fastctx settings {}: the file is not valid UTF-8 ({error})",
                crate::paths::display_path(&paths.fastctx_config)
            )
        })?;
        let source_schema = source_schema_version(&paths.fastctx_config, source)?;
        let mut settings = decode_source(&paths.fastctx_config, source)?;
        let migration_notice = settings.tool_budget_epoch.unwrap_or(0) < TOOL_BUDGET_EPOCH;
        if migration_notice {
            // This migration intentionally drops customized shares along with old defaults: a
            // share is relative, so keeping one across a change of the global budget silently
            // rescales it. The user-visible notice makes that product decision explicit.
            settings.tool_budgets = ToolBudgetPreferences::default();
        }
        let current_version = env!("CARGO_PKG_VERSION");
        let watermark_changed = settings.last_seen_version.as_deref() != Some(current_version);
        let schema_changed = source_schema != CURRENT_SCHEMA_VERSION;
        if !migration_notice && !watermark_changed && !schema_changed {
            return Ok(StartupSettings {
                settings,
                migration_notice: false,
            });
        }
        settings.last_seen_version = Some(current_version.to_string());
        settings.tool_budget_epoch = Some(TOOL_BUDGET_EPOCH);
        let bytes = if schema_changed {
            encode(&settings)?
        } else {
            encode_startup_normalization(&paths.fastctx_config, source, migration_notice)?
        };
        let change = transaction::FileChange {
            target: paths.fastctx_config.clone(),
            original: Some(original),
            action: transaction::FileAction::Write(bytes),
            unix_mode: transaction::existing_unix_mode(&paths.fastctx_config).or(Some(0o600)),
            locked_binary_fallback: false,
        };
        match transaction::commit(&[change]) {
            Ok(()) => {
                return Ok(StartupSettings {
                    settings,
                    migration_notice,
                });
            }
            Err(_) if attempt + 1 < MAX_COMMIT_ATTEMPTS => {
                // Another control process may have normalized the same file. Re-read the exact
                // current snapshot; deterministic permission or shape failures surface below.
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the bounded startup-normalization loop always returns")
}

/// Inspects raw limit values so Status can report fallback rather than silently hiding it.
pub fn job_limit_status(paths: &ControlPaths) -> Result<JobLimitStatus, String> {
    let settings = load(paths)?;
    let source = match fs::read_to_string(&paths.fastctx_config) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "Cannot read fastctx settings {}: {error}",
                crate::paths::display_path(&paths.fastctx_config)
            ));
        }
    };
    let document = if source.is_empty() {
        None
    } else {
        Some(source.parse::<toml_edit::DocumentMut>().map_err(|error| {
            format!(
                "Cannot parse fastctx settings {}: {error}. Repair or remove the file and retry.",
                crate::paths::display_path(&paths.fastctx_config)
            )
        })?)
    };
    let invalid = |key: &str, maximum: Option<i64>| {
        document
            .as_ref()
            .and_then(|document| document.get("fastshell"))
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|table| table.get(key))
            .is_some_and(|item| {
                item.as_integer().is_none_or(|value| {
                    value <= 0 || maximum.is_some_and(|maximum| value > maximum)
                })
            })
    };
    Ok(JobLimitStatus {
        job_storage_limit_mib: settings.fastshell.job_storage_limit_mib,
        max_running_jobs: settings.fastshell.max_running_jobs,
        job_list_limit: settings.fastshell.job_list_limit,
        storage_limit_fell_back: invalid("job_storage_limit_mib", None),
        running_limit_fell_back: invalid("max_running_jobs", None),
        list_limit_fell_back: invalid("job_list_limit", Some(MAX_JOB_LIST_LIMIT as i64)),
    })
}

/// Inspects raw update settings so Status can report an invalid-source fallback.
pub fn update_settings_status(paths: &ControlPaths) -> Result<UpdateSettingsStatus, String> {
    let settings = load(paths)?;
    let source = match fs::read_to_string(&paths.fastctx_config) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "Cannot read fastctx settings {}: {error}",
                crate::paths::display_path(&paths.fastctx_config)
            ));
        }
    };
    let source_fell_back = if source.is_empty() {
        false
    } else {
        let document = source.parse::<toml_edit::DocumentMut>().map_err(|error| {
            format!(
                "Cannot parse fastctx settings {}: {error}. Repair or remove the file and retry.",
                crate::paths::display_path(&paths.fastctx_config)
            )
        })?;
        document
            .get("update")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|table| table.get("source"))
            .is_some_and(|item| item.as_str().and_then(UpdateSource::parse).is_none())
    };
    Ok(UpdateSettingsStatus {
        auto_check: settings.update.auto_check,
        source: settings.update.source,
        source_fell_back,
    })
}

/// Resolves the persisted search CPU limit without hiding an out-of-range value.
pub fn search_parallelism_status(paths: &ControlPaths) -> Result<SearchParallelismStatus, String> {
    let settings = load(paths)?;
    let configured = settings.search.max_cpu_cores;
    let available = search_parallelism::detected_available();
    let effective = search_parallelism::resolve(configured)
        .ok()
        .map(|resolved| resolved.effective);
    Ok(SearchParallelismStatus {
        available,
        configured,
        effective,
    })
}

impl FastCtxSettings {
    /// Resolves the effective search parallelism or rejects an invalid explicit limit.
    pub(crate) fn search_parallelism(&self) -> Result<SearchParallelism, String> {
        search_parallelism::resolve(self.search.max_cpu_cores)
            .map_err(|error| format!("search.max_cpu_cores {error}"))
    }

    /// Resolves the replace limit or reports the exact persisted key that needs repair.
    pub(crate) fn replace_file_limit_mib(&self) -> Result<u64, String> {
        self.replace.resolved_file_limit_mib()
    }
}

/// Restores every user preference while retaining the Apply ownership receipt.
pub(crate) fn reset_user_preferences(settings: &FastCtxSettings) -> FastCtxSettings {
    FastCtxSettings {
        last_seen_version: settings.last_seen_version.clone(),
        // A reset lands exactly on this generation's defaults, so stamp it rather than leaving the
        // file looking unreconciled and re-showing the migration notice on the next launch.
        tool_budget_epoch: Some(TOOL_BUDGET_EPOCH),
        applied: settings.applied.clone(),
        installation: settings.installation.clone(),
        target_preferences: settings.target_preferences.clone(),
        applied_targets: settings.applied_targets.clone(),
        ..FastCtxSettings::default()
    }
}

fn search_parallelism_repair_hint() -> String {
    format!(
        "For search.max_cpu_cores, use a whole number from 1..={} or remove the key for automatic mode. ",
        search_parallelism::detected_available()
    )
}

fn replace_limit_repair_hint() -> String {
    format!(
        "For replace.max_file_size_mib, use a whole number from {MIN_REPLACE_FILE_LIMIT_MIB}..={MAX_REPLACE_FILE_LIMIT_MIB} MiB. "
    )
}

fn source_mentions_search_parallelism(source: &str) -> bool {
    let mut in_search_table = false;
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_search_table = line == "[search]";
            continue;
        }
        let Some((key, _value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key == "search.max_cpu_cores" || (in_search_table && key == "max_cpu_cores") {
            return true;
        }
    }
    false
}

fn source_mentions_replace_limit(source: &str) -> bool {
    let mut in_replace_table = false;
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_replace_table = line == "[replace]";
            continue;
        }
        let Some((key, _value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key == "replace.max_file_size_mib" || (in_replace_table && key == "max_file_size_mib") {
            return true;
        }
    }
    false
}

fn validate_search_parallelism_type(
    document: &toml_edit::DocumentMut,
    path: &Path,
) -> Result<(), String> {
    let Some(search) = document.get("search") else {
        return Ok(());
    };
    let Some(table) = search.as_table_like() else {
        return Err(format!(
            "Cannot parse fastctx settings {}: search must be a table. {}Repair the file and retry.",
            crate::paths::display_path(path),
            search_parallelism_repair_hint()
        ));
    };
    if table
        .get("max_cpu_cores")
        .is_some_and(|value| value.as_integer().is_none())
    {
        return Err(format!(
            "Cannot parse fastctx settings {}: search.max_cpu_cores must be an integer. {}Repair the file and retry.",
            crate::paths::display_path(path),
            search_parallelism_repair_hint()
        ));
    }
    Ok(())
}

fn validate_replace_limit_type(
    document: &toml_edit::DocumentMut,
    path: &Path,
) -> Result<(), String> {
    let Some(replace) = document.get("replace") else {
        return Ok(());
    };
    let Some(table) = replace.as_table_like() else {
        return Err(format!(
            "Cannot parse fastctx settings {}: replace must be a table. {}Repair the file and retry.",
            crate::paths::display_path(path),
            replace_limit_repair_hint()
        ));
    };
    if table
        .get("max_file_size_mib")
        .is_some_and(|value| value.as_integer().is_none())
    {
        return Err(format!(
            "Cannot parse fastctx settings {}: replace.max_file_size_mib must be an integer. {}Repair the file and retry.",
            crate::paths::display_path(path),
            replace_limit_repair_hint()
        ));
    }
    Ok(())
}

/// Loads configuration from a supplied path for tests and migrations.
pub fn load_from(path: &Path) -> Result<FastCtxSettings, String> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FastCtxSettings::default());
        }
        Err(error) => {
            return Err(format!(
                "Cannot read fastctx settings {}: {error}",
                crate::paths::display_path(path)
            ));
        }
    };
    decode_source(path, &source)
}

fn decode_source(path: &Path, source: &str) -> Result<FastCtxSettings, String> {
    let document = source.parse::<toml_edit::DocumentMut>().map_err(|error| {
        let mut hint = String::new();
        if source_mentions_search_parallelism(source) {
            hint.push_str(&search_parallelism_repair_hint());
        }
        if source_mentions_replace_limit(source) {
            hint.push_str(&replace_limit_repair_hint());
        }
        format!(
            "Cannot parse fastctx settings {}: {error}. {hint}Repair or remove the file and retry.",
            crate::paths::display_path(path)
        )
    })?;
    let schema_version = document
        .get("schema_version")
        .ok_or_else(|| {
            format!(
                "Cannot parse fastctx settings {}: schema_version is missing. Repair or remove the file and retry.",
                crate::paths::display_path(path)
            )
        })?
        .as_integer()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            format!(
                "Cannot parse fastctx settings {}: schema_version must be a non-negative integer. Repair the file and retry.",
                crate::paths::display_path(path)
            )
        })?;
    if schema_version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "Cannot write fastctx settings {}: schema_version {} was written by a newer fastctx. Upgrade fastctx and retry.",
            crate::paths::display_path(path),
            schema_version
        ));
    }
    if !matches!(
        schema_version,
        LEGACY_SCHEMA_VERSION | SINGLE_TARGET_SCHEMA_VERSION | CURRENT_SCHEMA_VERSION
    ) {
        return Err(format!(
            "Unsupported fastctx settings schema_version {schema_version} in {}. Upgrade fastctx or repair the file.",
            crate::paths::display_path(path)
        ));
    }
    validate_search_parallelism_type(&document, path)?;
    validate_replace_limit_type(&document, path)?;
    let mut settings: FastCtxSettings = toml_edit::de::from_str(source).map_err(|error| {
        format!(
            "Cannot parse fastctx settings {}: {error}. Repair or remove the file and retry.",
            crate::paths::display_path(path)
        )
    })?;
    settings.normalize_receipts_after_decode(schema_version)?;
    if let Some(language) = settings.language.as_deref()
        && !ALL_LANGUAGES
            .iter()
            .any(|supported| supported.code() == language)
    {
        let codes = ALL_LANGUAGES
            .iter()
            .map(|supported| supported.code())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Unsupported fastctx language \"{language}\" in {}. Use one of: {codes}.",
            crate::paths::display_path(path)
        ));
    }
    Ok(settings)
}

fn source_schema_version(path: &Path, source: &str) -> Result<u32, String> {
    let document = source.parse::<toml_edit::DocumentMut>().map_err(|error| {
        format!(
            "Cannot parse fastctx settings {}: {error}. Repair or remove the file and retry.",
            crate::paths::display_path(path)
        )
    })?;
    document
        .get("schema_version")
        .and_then(toml_edit::Item::as_integer)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            format!(
                "Cannot parse fastctx settings {}: schema_version is missing or invalid.",
                crate::paths::display_path(path)
            )
        })
}

fn encode_startup_normalization(
    path: &Path,
    source: &str,
    reset_tool_budgets: bool,
) -> Result<Vec<u8>, String> {
    let mut document = source.parse::<toml_edit::DocumentMut>().map_err(|error| {
        format!(
            "Cannot normalize fastctx settings {}: {error}. No settings were changed.",
            crate::paths::display_path(path)
        )
    })?;
    document["schema_version"] = toml_edit::value(i64::from(CURRENT_SCHEMA_VERSION));
    document["last_seen_version"] = toml_edit::value(env!("CARGO_PKG_VERSION"));
    document["tool_budget_epoch"] = toml_edit::value(i64::from(TOOL_BUDGET_EPOCH));
    if reset_tool_budgets {
        document.remove("tool_budgets");
    }
    let mut normalized = document.to_string();
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    Ok(normalized.into_bytes())
}

/// Encodes configuration as stable UTF-8 TOML.
pub fn encode(settings: &FastCtxSettings) -> Result<Vec<u8>, String> {
    if settings.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "Refusing to write fastctx settings schema_version {}; this fastctx only writes schema_version {CURRENT_SCHEMA_VERSION}.",
            settings.schema_version
        ));
    }
    settings
        .search_parallelism()
        .map_err(|error| format!("Cannot encode fastctx settings: {error}."))?;
    settings
        .replace_file_limit_mib()
        .map_err(|error| format!("Cannot encode fastctx settings: {error}."))?;
    let mut serializable = settings.clone();
    serializable.sync_codex_receipt_to_v2();
    serializable.schema_version = CURRENT_SCHEMA_VERSION;
    let mut source = toml_edit::ser::to_string_pretty(&serializable)
        .map_err(|error| format!("Cannot encode fastctx settings: {error}"))?;
    if !source.ends_with('\n') {
        source.push('\n');
    }
    Ok(source.into_bytes())
}

/// Atomically saves FastCtx configuration.
pub fn save(paths: &ControlPaths, settings: &FastCtxSettings) -> Result<bool, String> {
    let bytes = encode(settings)?;
    let original = transaction::read_snapshot(&paths.fastctx_config)?;
    if original.as_deref() == Some(bytes.as_slice()) {
        crate::shell::jobs::reap(paths).map_err(|error| {
            format!(
                "Settings were unchanged, but finished job records could not be reaped: {error}"
            )
        })?;
        return Ok(false);
    }
    fs::create_dir_all(&paths.fastctx_dir).map_err(|error| {
        format!(
            "Cannot create fastctx settings directory {}: {error}",
            crate::paths::display_path(&paths.fastctx_dir)
        )
    })?;
    transaction::atomic_replace(&paths.fastctx_config, &bytes, None, false)?;
    crate::shell::jobs::reap(paths).map_err(|error| {
        format!("Settings were saved, but finished job records could not be reaped: {error}")
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{Tier, load_for_startup, load_from};
    use crate::control::paths::ControlPaths;

    #[test]
    fn startup_migrates_an_existing_unstamped_config_and_overwrites_custom_budgets_once() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        std::fs::write(
            &paths.fastctx_config,
            concat!(
                "schema_version = 1\n",
                "language = \"en\"\n",
                "tier = \"high\"\n",
                "\n[tool_budgets]\n",
                "read = \"percent25\"\n",
                "grep = \"percent75\"\n",
                "glob = \"inherit\"\n",
                "run = \"percent75\"\n",
                "job_output = \"inherit\"\n",
            ),
        )
        .unwrap();

        let startup = load_for_startup(&paths).unwrap();
        assert!(startup.migration_notice);
        assert_eq!(
            startup.settings.tool_budgets,
            super::ToolBudgetPreferences::default()
        );
        assert_eq!(startup.settings.tier, Tier::High);
        assert_eq!(
            startup.settings.last_seen_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        let persisted = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(
            persisted.contains(&format!(
                "last_seen_version = \"{}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "{persisted}"
        );
        assert!(!persisted.contains("[tool_budgets]"), "{persisted}");

        let after_first = std::fs::read(&paths.fastctx_config).unwrap();
        let second = load_for_startup(&paths).unwrap();
        assert!(!second.migration_notice);
        assert_eq!(second.settings, startup.settings);
        assert_eq!(std::fs::read(&paths.fastctx_config).unwrap(), after_first);
    }

    #[test]
    fn future_or_missing_schema_versions_are_read_only_failures() {
        let temp = tempfile::tempdir().unwrap();
        let future = temp.path().join("future.toml");
        std::fs::write(&future, b"schema_version = 999\nlanguage = \"en\"\n").unwrap();
        let error = load_from(&future).unwrap_err();
        assert!(error.contains("written by a newer fastctx"), "{error}");
        assert_eq!(
            std::fs::read(&future).unwrap(),
            b"schema_version = 999\nlanguage = \"en\"\n"
        );

        let missing = temp.path().join("missing-version.toml");
        std::fs::write(&missing, b"language = \"en\"\n").unwrap();
        let error = load_from(&missing).unwrap_err();
        assert!(error.contains("schema_version is missing"), "{error}");
        assert_eq!(std::fs::read(&missing).unwrap(), b"language = \"en\"\n");
    }
}
