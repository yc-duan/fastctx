//! Stable format and atomic I/O for `~/.fastctx/config.toml`.

use crate::control::agents::InsertedSeparator;
use crate::control::i18n::ALL_LANGUAGES;
use crate::control::paths::ControlPaths;
use crate::control::transaction;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const CURRENT_SCHEMA_VERSION: u32 = 1;
const LEGACY_SCHEMA_VERSION: u32 = 0;
/// Default current-user disk allowance for retained background-job records.
pub const DEFAULT_JOB_STORAGE_LIMIT_MIB: u64 = 1_024;
/// Default current-user number of simultaneously running background jobs.
pub const DEFAULT_MAX_RUNNING_JOBS: u64 = 128;
/// Default number of background-job records returned by one `job_list` call.
pub const DEFAULT_JOB_LIST_LIMIT: u64 = 20;
/// Largest configurable page size accepted by `job_list`.
pub const MAX_JOB_LIST_LIMIT: u64 = 100;

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
    /// Codex default of 10k with an 8.5k FastCtx budget.
    #[default]
    Standard,
    /// Codex 16k with a 13.6k FastCtx budget.
    High,
    /// Codex 25k with a 21.25k FastCtx budget.
    ExtraHigh,
}

impl Tier {
    /// Host token limit written to Codex.
    pub const fn host_limit(self) -> i64 {
        match self {
            Self::Standard => 10_000,
            Self::High => 16_000,
            Self::ExtraHigh => 25_000,
        }
    }

    /// Global token budget written to the FastCtx environment.
    pub const fn fastctx_budget(self) -> usize {
        match self {
            Self::Standard => 8_500,
            Self::High => 13_600,
            Self::ExtraHigh => 21_250,
        }
    }

    /// Stable English identifier used by configuration and CLI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::High => "high",
            Self::ExtraHigh => "extra-high",
        }
    }

    /// Tier proper name shown by the UI and kept in English in every language.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::High => "High",
            Self::ExtraHigh => "Extra High",
        }
    }

    /// Selects the previous tier cyclically.
    pub const fn previous(self) -> Self {
        match self {
            Self::Standard => Self::ExtraHigh,
            Self::High => Self::Standard,
            Self::ExtraHigh => Self::High,
        }
    }

    /// Selects the next tier cyclically.
    pub const fn next(self) -> Self {
        match self {
            Self::Standard => Self::High,
            Self::High => Self::ExtraHigh,
            Self::ExtraHigh => Self::Standard,
        }
    }
}

/// Per-tool tier relative to the global budget.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolBudgetLevel {
    /// Omit the per-tool environment variable so the server inherits the global value.
    #[default]
    Inherit,
    /// Seventy-five percent of the global budget.
    Percent75,
    /// Fifty percent of the global budget.
    Percent50,
    /// Twenty-five percent of the global budget.
    Percent25,
}

impl ToolBudgetLevel {
    /// Returns the concrete budget to write, or `None` for inheritance.
    pub fn resolve(self, global: usize) -> Option<usize> {
        let percent = match self {
            Self::Inherit => return None,
            Self::Percent75 => 75,
            Self::Percent50 => 50,
            Self::Percent25 => 25,
        };
        let raw = (global * percent + 50) / 100;
        Some(((raw + 50) / 100) * 100)
    }

    /// Stable English label shown by the UI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Percent75 => "75%",
            Self::Percent50 => "50%",
            Self::Percent25 => "25%",
        }
    }

    /// Selects the previous tier cyclically.
    pub const fn previous(self) -> Self {
        match self {
            Self::Inherit => Self::Percent25,
            Self::Percent75 => Self::Inherit,
            Self::Percent50 => Self::Percent75,
            Self::Percent25 => Self::Percent50,
        }
    }

    /// Selects the next tier cyclically.
    pub const fn next(self) -> Self {
        match self {
            Self::Inherit => Self::Percent75,
            Self::Percent75 => Self::Percent50,
            Self::Percent50 => Self::Percent25,
            Self::Percent25 => Self::Inherit,
        }
    }
}

/// Per-tool budget choices for the five long-output tools.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ToolBudgets {
    /// Relative budget for read.
    pub read: ToolBudgetLevel,
    /// Relative budget for grep.
    pub grep: ToolBudgetLevel,
    /// Relative budget for glob.
    pub glob: ToolBudgetLevel,
    /// Relative budget for run; effective only when the shell group is enabled.
    pub run: ToolBudgetLevel,
    /// Relative budget for job_output; effective only when the shell group is enabled.
    pub job_output: ToolBudgetLevel,
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
    // Path-only lists are low-density, so a tighter default only affects unusually broad patterns.
    fn default() -> Self {
        Self {
            read: ToolBudgetLevel::Inherit,
            grep: ToolBudgetLevel::Inherit,
            glob: ToolBudgetLevel::Percent50,
            run: ToolBudgetLevel::Inherit,
            job_output: ToolBudgetLevel::Inherit,
        }
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
    /// Legacy receipt field accepted so older installations can be re-applied safely.
    #[serde(default, skip_serializing)]
    pub fastedit_enabled: bool,
    /// Whether Apply created `~/.codex/`; Unapply removes that owned shell only while it remains empty.
    #[serde(default)]
    pub codex_dir_created: bool,
    /// Ownership receipt for Codex config.
    pub codex_config: ManagedFileRecord,
    /// Ownership receipt for Codex AGENTS.md.
    pub codex_agents: ManagedFileRecord,
    /// Leading AGENTS separator inserted by the first Apply, used as reverse-operation ownership evidence only without drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_agents_inserted_separator: Option<InsertedSeparator>,
    /// Content hash of the self-installed binary.
    pub binary_sha256: String,
}

/// FastCtx's own configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct FastCtxSettings {
    /// Configuration format version.
    pub schema_version: u32,
    /// TUI language; absence means first-run selection is incomplete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Host tier used by the next Apply.
    pub tier: Tier,
    /// Advanced per-tool tiers used by the next Apply.
    pub tool_budgets: ToolBudgets,
    /// Optional fastshell server, disabled by default.
    pub fastshell: FastShellSettings,
    /// Machine-level update preferences, effective immediately when saved.
    pub update: UpdateSettings,
    /// Legacy config key accepted but omitted from every newly written settings file.
    #[serde(default, skip_serializing)]
    pub fastedit: FeatureToggle,
    /// Receipt for the most recent successful Apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied: Option<AppliedRecord>,
}

impl Default for FastCtxSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            language: None,
            tier: Tier::Standard,
            tool_budgets: ToolBudgets::default(),
            fastshell: FastShellSettings::default(),
            update: UpdateSettings::default(),
            fastedit: FeatureToggle::default(),
            applied: None,
        }
    }
}

/// Loads FastCtx configuration, returning defaults when the file does not exist.
pub fn load(paths: &ControlPaths) -> Result<FastCtxSettings, String> {
    load_from(&paths.fastctx_config)
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
    let document = source.parse::<toml_edit::DocumentMut>().map_err(|error| {
        format!(
            "Cannot parse fastctx settings {}: {error}. Repair or remove the file and retry.",
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
        LEGACY_SCHEMA_VERSION | CURRENT_SCHEMA_VERSION
    ) {
        return Err(format!(
            "Unsupported fastctx settings schema_version {schema_version} in {}. Upgrade fastctx or repair the file.",
            crate::paths::display_path(path)
        ));
    }
    let mut settings: FastCtxSettings = toml_edit::de::from_str(&source).map_err(|error| {
        format!(
            "Cannot parse fastctx settings {}: {error}. Repair or remove the file and retry.",
            crate::paths::display_path(path)
        )
    })?;
    if schema_version == LEGACY_SCHEMA_VERSION {
        settings.schema_version = CURRENT_SCHEMA_VERSION;
    }
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

/// Encodes configuration as stable UTF-8 TOML.
pub fn encode(settings: &FastCtxSettings) -> Result<Vec<u8>, String> {
    if settings.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "Refusing to write fastctx settings schema_version {}; this fastctx only writes schema_version {CURRENT_SCHEMA_VERSION}.",
            settings.schema_version
        ));
    }
    let mut source = toml_edit::ser::to_string_pretty(settings)
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
    use super::{
        CURRENT_SCHEMA_VERSION, DEFAULT_JOB_LIST_LIMIT, DEFAULT_JOB_STORAGE_LIMIT_MIB,
        DEFAULT_MAX_RUNNING_JOBS, MAX_JOB_LIST_LIMIT, ManagedFileRecord, Tier, UpdateSource,
        encode, job_limit_status, load_from, update_settings_status,
    };
    use crate::control::paths::ControlPaths;

    #[test]
    fn tier_budget_mapping_preserves_fifteen_percent_host_headroom() {
        let expected = [
            (Tier::Standard, 10_000, 8_500),
            (Tier::High, 16_000, 13_600),
            (Tier::ExtraHigh, 25_000, 21_250),
        ];

        for (tier, host_limit, fastctx_budget) in expected {
            assert_eq!(tier.host_limit(), host_limit);
            assert_eq!(tier.fastctx_budget(), fastctx_budget);
        }
    }

    #[test]
    fn invalid_persisted_language_is_an_explicit_configuration_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, b"schema_version = 1\nlanguage = \"bogus\"\n").unwrap();

        let error = load_from(&path).unwrap_err();
        assert!(error.contains("Unsupported fastctx language \"bogus\""));
        assert!(error.contains("zh-CN"));
        assert!(error.contains("uk"));
    }

    #[test]
    fn invalid_update_source_falls_back_to_auto_and_remains_diagnosable() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        std::fs::write(
            &paths.fastctx_config,
            b"schema_version = 1\n\n[update]\nauto_check = false\nsource = \"untrusted-mirror\"\n",
        )
        .unwrap();

        let settings = load_from(&paths.fastctx_config).unwrap();
        assert!(!settings.update.auto_check);
        assert_eq!(settings.update.source, UpdateSource::Auto);
        let status = update_settings_status(&paths).unwrap();
        assert!(!status.auto_check);
        assert_eq!(status.source, UpdateSource::Auto);
        assert!(status.source_fell_back);
        let encoded = String::from_utf8(encode(&settings).unwrap()).unwrap();
        assert!(encoded.contains("[update]\nauto_check = false\nsource = \"auto\""));
    }

    #[test]
    fn invalid_current_user_job_limits_fall_back_and_remain_diagnosable() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        std::fs::write(
            &paths.fastctx_config,
            b"schema_version = 1\n\n[fastshell]\njob_storage_limit_mib = 0\nmax_running_jobs = -2\njob_list_limit = 101\n",
        )
        .unwrap();

        let settings = load_from(&paths.fastctx_config).unwrap();
        assert_eq!(
            settings.fastshell.job_storage_limit_mib,
            DEFAULT_JOB_STORAGE_LIMIT_MIB
        );
        assert_eq!(
            settings.fastshell.max_running_jobs,
            DEFAULT_MAX_RUNNING_JOBS
        );
        assert_eq!(settings.fastshell.job_list_limit, DEFAULT_JOB_LIST_LIMIT);
        let status = job_limit_status(&paths).unwrap();
        assert!(status.storage_limit_fell_back);
        assert!(status.running_limit_fell_back);
        assert!(status.list_limit_fell_back);
        assert_eq!(status.job_storage_limit_mib, DEFAULT_JOB_STORAGE_LIMIT_MIB);
        assert_eq!(status.max_running_jobs, DEFAULT_MAX_RUNNING_JOBS);
        assert_eq!(status.job_list_limit, DEFAULT_JOB_LIST_LIMIT);
    }

    #[test]
    fn job_list_limit_accepts_both_persisted_contract_boundaries() {
        for expected in [1, MAX_JOB_LIST_LIMIT] {
            let temp = tempfile::tempdir().unwrap();
            let paths = ControlPaths::for_home(temp.path());
            std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
            std::fs::write(
                &paths.fastctx_config,
                format!("schema_version = 1\n\n[fastshell]\njob_list_limit = {expected}\n"),
            )
            .unwrap();

            let settings = load_from(&paths.fastctx_config).unwrap();
            assert_eq!(settings.fastshell.job_list_limit, expected);
            let status = job_limit_status(&paths).unwrap();
            assert_eq!(status.job_list_limit, expected);
            assert!(!status.list_limit_fell_back);
        }
    }

    #[test]
    fn legacy_managed_file_receipt_defaults_missing_existence_to_false() {
        let record: ManagedFileRecord = toml_edit::de::from_str(
            "path = \"C:/Users/example/.codex/AGENTS.md\"\napplied_sha256 = \"abc123\"\n",
        )
        .unwrap();

        assert!(!record.original_existed);
    }

    #[test]
    fn known_legacy_settings_migrate_in_memory_without_overwriting_the_source() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let original = b"schema_version = 0\ntier = \"high\"\n";
        std::fs::write(&path, original).unwrap();

        let settings = load_from(&path).unwrap();
        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.tier, Tier::High);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(
            encode(&settings)
                .unwrap()
                .starts_with(b"schema_version = 1\n")
        );
    }

    #[test]
    fn legacy_fastedit_keys_are_read_but_never_written_back() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            b"schema_version = 1\n[fastedit]\nenabled = true\n[applied]\napplied_at_utc = '2026-07-16T00:00:00Z'\nversion = '0.1.0'\ncommand = 'fastctx'\ntier = 'standard'\ntool_output_token_limit = 10000\nprevious_token_limit_present = false\nfastctx_token_budget = 8500\nfastedit_enabled = true\nbinary_sha256 = 'abc'\n[applied.tool_budgets]\nread='inherit'\ngrep='inherit'\nglob='percent50'\nrun='inherit'\njob_output='inherit'\n[applied.codex_config]\npath='config.toml'\noriginal_existed=true\napplied_sha256='abc'\n[applied.codex_agents]\npath='AGENTS.md'\noriginal_existed=true\napplied_sha256='def'\n",
        )
        .unwrap();

        let settings = load_from(&path).unwrap();
        assert!(settings.fastedit.enabled);
        assert!(settings.applied.as_ref().unwrap().fastedit_enabled);
        let encoded = String::from_utf8(encode(&settings).unwrap()).unwrap();
        assert!(!encoded.contains("fastedit"), "{encoded}");
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
