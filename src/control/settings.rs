//! Stable format and atomic I/O for `~/.fastctx/config.toml`.

use crate::control::agents::InsertedSeparator;
use crate::control::i18n::ALL_LANGUAGES;
use crate::control::paths::ControlPaths;
use crate::control::transaction;
use crate::search_parallelism::{self, SearchParallelism};
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

/// Current-user grep/glob CPU settings, read directly by each newly started server.
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
    /// Codex factory default of 10k with an 8.5k FastCtx budget.
    Compact,
    /// Recommended 20k host limit with a 17k FastCtx budget.
    #[default]
    Standard,
    /// Codex 30k with a 25.5k FastCtx budget.
    #[serde(alias = "extra-high")]
    #[value(alias = "extra-high")]
    High,
}

impl Tier {
    /// Host token limit written to Codex.
    pub const fn host_limit(self) -> i64 {
        match self {
            Self::Compact => 10_000,
            Self::Standard => 20_000,
            Self::High => 30_000,
        }
    }

    /// Global token budget written to the FastCtx environment.
    pub const fn fastctx_budget(self) -> usize {
        match self {
            Self::Compact => 8_500,
            Self::Standard => 17_000,
            Self::High => 25_500,
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
    fn default() -> Self {
        Self {
            read: ToolBudgetLevel::Inherit,
            grep: ToolBudgetLevel::Percent50,
            glob: ToolBudgetLevel::Percent25,
            run: ToolBudgetLevel::Percent25,
            job_output: ToolBudgetLevel::Percent25,
        }
    }
}

impl ToolBudgets {
    fn is_default(&self) -> bool {
        *self == Self::default()
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
    /// TUI language; absence means first-run selection is incomplete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Host tier used by the next Apply.
    pub tier: Tier,
    /// Advanced per-tool tiers used by the next Apply.
    #[serde(skip_serializing_if = "ToolBudgets::is_default")]
    pub tool_budgets: ToolBudgets,
    /// Optional fastshell server, disabled by default.
    pub fastshell: FastShellSettings,
    /// Machine-level update preferences, effective immediately when saved.
    pub update: UpdateSettings,
    /// Current-user grep/glob CPU limit, effective for newly started server processes.
    #[serde(skip_serializing_if = "SearchSettings::is_default")]
    pub search: SearchSettings,
    /// Legacy config key accepted but omitted from every newly written settings file.
    #[serde(default, skip_serializing)]
    pub fastedit: FeatureToggle,
    /// Receipt for the most recent successful Apply.
    #[serde(skip_serializing_if = "Option::is_none")]
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
            language: None,
            tier: Tier::Standard,
            tool_budgets: ToolBudgets::default(),
            fastshell: FastShellSettings::default(),
            update: UpdateSettings::default(),
            search: SearchSettings::default(),
            fastedit: FeatureToggle::default(),
            applied: None,
        }
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
        let mut settings = decode_source(&paths.fastctx_config, source)?;
        let migration_notice = settings.last_seen_version.is_none();
        if migration_notice {
            // This one-time migration intentionally replaces customized values as well as old
            // defaults; the user-visible notice makes that product decision explicit.
            settings.tool_budgets = ToolBudgets::default();
        }
        let current_version = env!("CARGO_PKG_VERSION");
        let watermark_changed = settings.last_seen_version.as_deref() != Some(current_version);
        if !migration_notice && !watermark_changed {
            return Ok(StartupSettings {
                settings,
                migration_notice: false,
            });
        }
        settings.last_seen_version = Some(current_version.to_string());
        let bytes = encode_startup_normalization(&paths.fastctx_config, source, migration_notice)?;
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
}

/// Restores every user preference while retaining the Apply ownership receipt.
pub(crate) fn reset_user_preferences(settings: &FastCtxSettings) -> FastCtxSettings {
    FastCtxSettings {
        last_seen_version: settings.last_seen_version.clone(),
        applied: settings.applied.clone(),
        ..FastCtxSettings::default()
    }
}

fn search_parallelism_repair_hint() -> String {
    format!(
        "For search.max_cpu_cores, use a whole number from 1..={} or remove the key for automatic mode. ",
        search_parallelism::detected_available()
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
        let hint = if source_mentions_search_parallelism(source) {
            search_parallelism_repair_hint()
        } else {
            String::new()
        };
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
        LEGACY_SCHEMA_VERSION | CURRENT_SCHEMA_VERSION
    ) {
        return Err(format!(
            "Unsupported fastctx settings schema_version {schema_version} in {}. Upgrade fastctx or repair the file.",
            crate::paths::display_path(path)
        ));
    }
    validate_search_parallelism_type(&document, path)?;
    let mut settings: FastCtxSettings = toml_edit::de::from_str(source).map_err(|error| {
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
        AppliedRecord, CURRENT_SCHEMA_VERSION, DEFAULT_JOB_LIST_LIMIT,
        DEFAULT_JOB_STORAGE_LIMIT_MIB, DEFAULT_MAX_RUNNING_JOBS, FastCtxSettings,
        MAX_JOB_LIST_LIMIT, ManagedFileRecord, Tier, ToolBudgetLevel, UpdateSource, encode,
        job_limit_status, load_for_startup, load_from, reset_user_preferences, save,
        search_parallelism_status, update_settings_status,
    };
    use crate::control::paths::ControlPaths;

    #[test]
    fn tier_budget_mapping_preserves_fifteen_percent_host_headroom() {
        let expected = [
            (Tier::Compact, 10_000, 8_500),
            (Tier::Standard, 20_000, 17_000),
            (Tier::High, 30_000, 25_500),
        ];

        for (tier, host_limit, fastctx_budget) in expected {
            assert_eq!(tier.host_limit(), host_limit);
            assert_eq!(tier.fastctx_budget(), fastctx_budget);
        }
    }

    #[test]
    fn tool_budget_defaults_match_the_recentered_output_contract() {
        let defaults = super::ToolBudgets::default();
        assert_eq!(defaults.read, ToolBudgetLevel::Inherit);
        assert_eq!(defaults.grep, ToolBudgetLevel::Percent50);
        assert_eq!(defaults.glob, ToolBudgetLevel::Percent25);
        assert_eq!(defaults.run, ToolBudgetLevel::Percent25);
        assert_eq!(defaults.job_output, ToolBudgetLevel::Percent25);
    }

    #[test]
    fn percentage_budgets_use_the_frozen_nearest_hundred_resolution() {
        let cases = [
            (8_500, [6_400, 4_300, 2_100]),
            (17_000, [12_800, 8_500, 4_300]),
            (25_500, [19_100, 12_800, 6_400]),
        ];
        for (global, expected) in cases {
            let actual = [
                ToolBudgetLevel::Percent75.resolve(global).unwrap(),
                ToolBudgetLevel::Percent50.resolve(global).unwrap(),
                ToolBudgetLevel::Percent25.resolve(global).unwrap(),
            ];
            assert_eq!(actual, expected, "global budget {global}");
            assert!(actual.into_iter().all(|budget| budget <= global));
        }
    }

    #[test]
    fn default_tool_budgets_are_omitted_but_explicit_choices_round_trip() {
        let defaults = FastCtxSettings::default();
        let encoded = String::from_utf8(encode(&defaults).unwrap()).unwrap();
        assert!(!encoded.contains("[tool_budgets]"), "{encoded}");

        let mut customized = defaults;
        customized.tool_budgets.grep = ToolBudgetLevel::Percent75;
        let encoded = encode(&customized).unwrap();
        let source = String::from_utf8(encoded.clone()).unwrap();
        assert!(
            source.contains("[tool_budgets]\nread = \"inherit\"\ngrep = \"percent75\""),
            "{source}"
        );

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, encoded).unwrap();
        assert_eq!(
            load_from(&path).unwrap().tool_budgets,
            customized.tool_budgets
        );
    }

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
        assert_eq!(startup.settings.tool_budgets, super::ToolBudgets::default());
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
    fn startup_leaves_an_already_stamped_config_and_its_explicit_budgets_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        let original = format!(
            concat!(
                "schema_version = 1\n",
                "last_seen_version = \"{}\"\n",
                "language = \"en\"\n",
                "\n[tool_budgets]\n",
                "grep = \"percent75\"\n",
            ),
            env!("CARGO_PKG_VERSION")
        );
        std::fs::write(&paths.fastctx_config, original.as_bytes()).unwrap();

        let startup = load_for_startup(&paths).unwrap();
        assert!(!startup.migration_notice);
        assert_eq!(
            startup.settings.tool_budgets.grep,
            ToolBudgetLevel::Percent75
        );
        assert_eq!(
            std::fs::read(&paths.fastctx_config).unwrap(),
            original.as_bytes()
        );
    }

    #[test]
    fn startup_advances_an_existing_watermark_without_reapplying_the_budget_migration() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        // A watermark that can never equal the crate version, so this test always exercises the
        // advance path instead of the unchanged-watermark early return. (2026-07-24)
        let original = concat!(
            "schema_version = 1\n",
            "last_seen_version = \"0.0.1\"\n",
            "language = \"en\"\n",
            "\n[tool_budgets]\n",
            "grep = \"percent75\"\n",
        );
        std::fs::write(&paths.fastctx_config, original).unwrap();

        let startup = load_for_startup(&paths).unwrap();
        assert!(!startup.migration_notice);
        assert_eq!(
            startup.settings.tool_budgets.grep,
            ToolBudgetLevel::Percent75
        );
        assert_eq!(
            startup.settings.last_seen_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        let persisted = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(persisted.contains("grep = \"percent75\""), "{persisted}");
        assert!(
            persisted.contains(&format!(
                "last_seen_version = \"{}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "{persisted}"
        );
    }

    #[test]
    fn startup_migration_preserves_an_invalid_search_limit_for_the_repair_ui() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        let source = concat!(
            "schema_version = 1\n",
            "language = \"en\"\n",
            "\n[tool_budgets]\n",
            "grep = \"percent75\"\n",
            "\n[search]\n",
            "max_cpu_cores = 0\n",
        );
        std::fs::write(&paths.fastctx_config, source).unwrap();

        let startup = load_for_startup(&paths).unwrap();
        assert!(startup.migration_notice);
        assert_eq!(startup.settings.search.max_cpu_cores, Some(0));
        assert_eq!(startup.settings.tool_budgets, super::ToolBudgets::default());
        let persisted = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(persisted.contains("max_cpu_cores = 0"), "{persisted}");
        assert!(!persisted.contains("[tool_budgets]"), "{persisted}");
        assert!(persisted.contains("last_seen_version"), "{persisted}");
    }

    #[test]
    fn startup_normalization_keeps_the_watermark_a_top_level_key_after_trailing_tables() {
        // Normalization inserts a brand-new top-level key into a document that already opened
        // tables. Rendering it after a table header would silently reparent it (update.
        // last_seen_version), leaving the top level unstamped and re-migrating on every launch.
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        std::fs::write(
            &paths.fastctx_config,
            concat!(
                "schema_version = 1\n",
                "language = \"en\"\n",
                "\n[tool_budgets]\n",
                "grep = \"percent75\"\n",
                "\n[update]\n",
                "auto_check = false\n",
                "\n[search]\n",
                "max_cpu_cores = 2\n",
            ),
        )
        .unwrap();

        assert!(load_for_startup(&paths).unwrap().migration_notice);

        let second = load_for_startup(&paths).unwrap();
        assert!(!second.migration_notice);
        assert_eq!(
            second.settings.last_seen_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert!(!second.settings.update.auto_check);
        assert_eq!(second.settings.search.max_cpu_cores, Some(2));
    }

    #[test]
    fn failed_startup_migration_leaves_the_original_config_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        let original = b"schema_version = 1\nlanguage = \"en\"\n";
        std::fs::write(&paths.fastctx_config, original).unwrap();
        let original_permissions = std::fs::metadata(&paths.fastctx_config)
            .unwrap()
            .permissions();
        let mut read_only = original_permissions.clone();
        read_only.set_readonly(true);
        std::fs::set_permissions(&paths.fastctx_config, read_only).unwrap();

        let result = load_for_startup(&paths);

        std::fs::set_permissions(&paths.fastctx_config, original_permissions).unwrap();
        let error = result.unwrap_err();
        assert!(error.contains("read-only file"), "{error}");
        assert_eq!(std::fs::read(&paths.fastctx_config).unwrap(), original);
    }

    #[test]
    fn startup_normalization_never_overwrites_malformed_or_future_schema_files() {
        let cases = [
            b"schema_version = 999\nlanguage = \"en\"\n".as_slice(),
            b"schema_version = 1\n[broken".as_slice(),
        ];
        for original in cases {
            let temp = tempfile::tempdir().unwrap();
            let paths = ControlPaths::for_home(temp.path());
            std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
            std::fs::write(&paths.fastctx_config, original).unwrap();

            assert!(load_for_startup(&paths).is_err());
            assert_eq!(std::fs::read(&paths.fastctx_config).unwrap(), original);
        }
    }

    #[test]
    fn fresh_startup_stamps_only_the_first_natural_save_and_never_misfires_as_an_upgrade() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());

        let mut startup = load_for_startup(&paths).unwrap();
        assert!(!startup.migration_notice);
        assert!(!paths.fastctx_config.exists());
        assert_eq!(
            startup.settings.last_seen_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        startup.settings.language = Some("en".to_string());
        save(&paths, &startup.settings).unwrap();

        let persisted = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(persisted.contains("last_seen_version"), "{persisted}");
        assert!(!load_for_startup(&paths).unwrap().migration_notice);
    }

    #[test]
    fn ordinary_save_preserves_a_missing_watermark_instead_of_consuming_migration() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let settings = FastCtxSettings {
            language: Some("en".to_string()),
            last_seen_version: None,
            ..FastCtxSettings::default()
        };

        save(&paths, &settings).unwrap();
        let persisted = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(!persisted.contains("last_seen_version"), "{persisted}");
        assert!(load_for_startup(&paths).unwrap().migration_notice);
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
    fn job_output_defaults_to_a_quarter_budget_without_overriding_an_explicit_legacy_choice() {
        assert_eq!(
            FastCtxSettings::default().tool_budgets.job_output,
            ToolBudgetLevel::Percent25
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            b"schema_version = 1\n[tool_budgets]\njob_output = 'inherit'\n",
        )
        .unwrap();
        assert_eq!(
            load_from(&path).unwrap().tool_budgets.job_output,
            ToolBudgetLevel::Inherit
        );
    }

    #[test]
    fn search_cpu_limit_is_omitted_by_default_and_valid_boundaries_round_trip() {
        let default = String::from_utf8(encode(&FastCtxSettings::default()).unwrap()).unwrap();
        assert!(!default.contains("[search]"), "{default}");

        let maximum = crate::search_parallelism::detected_available();
        let middle = (maximum / 2).max(1);
        for configured in [1, middle, maximum] {
            let temp = tempfile::tempdir().unwrap();
            let paths = ControlPaths::for_home(temp.path());
            std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
            std::fs::write(
                &paths.fastctx_config,
                format!("schema_version = 1\n\n[search]\nmax_cpu_cores = {configured}\n"),
            )
            .unwrap();

            let settings = load_from(&paths.fastctx_config).unwrap();
            assert_eq!(settings.search.max_cpu_cores, Some(configured as i64));
            let status = search_parallelism_status(&paths).unwrap();
            assert_eq!(status.available, maximum);
            assert_eq!(status.configured, Some(configured as i64));
            assert_eq!(status.effective, Some(configured));
            let encoded = String::from_utf8(encode(&settings).unwrap()).unwrap();
            assert!(
                encoded.contains(&format!("[search]\nmax_cpu_cores = {configured}")),
                "{encoded}"
            );
        }
    }

    #[test]
    fn search_cpu_limit_rejects_range_type_and_empty_errors_without_rewriting_source() {
        let maximum = crate::search_parallelism::detected_available();
        for configured in [-1_i64, 0, maximum as i64 + 1] {
            let temp = tempfile::tempdir().unwrap();
            let paths = ControlPaths::for_home(temp.path());
            std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
            let source = format!("schema_version = 1\n\n[search]\nmax_cpu_cores = {configured}\n");
            std::fs::write(&paths.fastctx_config, source.as_bytes()).unwrap();

            let mut settings = load_from(&paths.fastctx_config).unwrap();
            let status = search_parallelism_status(&paths).unwrap();
            assert_eq!(status.configured, Some(configured));
            assert_eq!(status.effective, None);
            settings.tier = Tier::High;
            let error = save(&paths, &settings).unwrap_err();
            assert!(error.contains(&format!("1..={maximum}")), "{error}");
            assert_eq!(
                std::fs::read(&paths.fastctx_config).unwrap(),
                source.as_bytes()
            );
        }

        for raw in ["\"four\"", "1.5", "true", ""] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("config.toml");
            let source = format!("schema_version = 1\n[search]\nmax_cpu_cores = {raw}\n");
            std::fs::write(&path, source.as_bytes()).unwrap();
            let error = load_from(&path).unwrap_err();
            for expected in [
                "Cannot parse fastctx settings".to_string(),
                "search.max_cpu_cores".to_string(),
                "whole number".to_string(),
                format!("1..={maximum}"),
                "automatic mode".to_string(),
            ] {
                assert!(error.contains(&expected), "missing {expected:?}: {error}");
            }
            assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
        }

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let unrelated = b"schema_version = 1\n[other]\nmax_cpu_cores =\n";
        std::fs::write(&path, unrelated).unwrap();
        let error = load_from(&path).unwrap_err();
        assert!(!error.contains("search.max_cpu_cores"), "{error}");
        assert!(!error.contains("automatic mode"), "{error}");
    }

    #[test]
    fn reset_restores_every_user_preference_and_preserves_the_apply_receipt() {
        let managed = |path: &str| ManagedFileRecord {
            path: path.to_string(),
            original_existed: true,
            applied_sha256: "managed-hash".to_string(),
        };
        let receipt = AppliedRecord {
            applied_at_utc: "2026-07-21T00:00:00Z".to_string(),
            version: "0.1.1".to_string(),
            command: "fastctx".to_string(),
            tier: Tier::High,
            tool_output_token_limit: 30_000,
            tool_timeout_sec: Some(300),
            previous_token_limit_present: true,
            previous_token_limit: Some(10_000),
            fastctx_token_budget: 25_500,
            tool_budgets: super::ToolBudgets::default(),
            fastshell_enabled: true,
            fastedit_enabled: false,
            codex_dir_created: true,
            codex_config: managed("config.toml"),
            codex_agents: managed("AGENTS.md"),
            codex_agents_inserted_separator: None,
            binary_sha256: "binary-hash".to_string(),
        };
        let mut settings = FastCtxSettings {
            language: Some("zh-CN".to_string()),
            tier: Tier::High,
            applied: Some(receipt.clone()),
            ..FastCtxSettings::default()
        };
        settings.tool_budgets.grep = ToolBudgetLevel::Percent25;
        settings.fastshell.enabled = true;
        settings.fastshell.job_storage_limit_mib = 4_096;
        settings.update.auto_check = false;
        settings.update.source = UpdateSource::Npmmirror;
        settings.search.max_cpu_cores = Some(1);

        let reset = reset_user_preferences(&settings);
        assert_eq!(reset.applied, Some(receipt));
        assert_eq!(
            FastCtxSettings {
                applied: None,
                ..reset.clone()
            },
            FastCtxSettings::default()
        );
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
    fn retired_extra_high_tier_migrates_by_intent_without_changing_the_schema() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let original = b"schema_version = 1\ntier = \"extra-high\"\n";
        std::fs::write(&path, original).unwrap();

        let settings = load_from(&path).unwrap();
        assert_eq!(settings.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(settings.tier, Tier::High);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let encoded = String::from_utf8(encode(&settings).unwrap()).unwrap();
        assert!(encoded.contains("tier = \"high\""), "{encoded}");
        assert!(!encoded.contains("extra-high"), "{encoded}");
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
