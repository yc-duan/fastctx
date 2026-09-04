//! Diagnosable Status/Doctor checks and a real stdio MCP handshake.

use crate::control::agents;
use crate::control::codex_config::{self, ExpectedConfig};
use crate::control::paths::ControlPaths;
use crate::control::provider::{self, CodexCompaction, EffectiveOutputMode, ProviderProvenance};
use crate::control::settings;
use crate::server::{FastCtxServer, ServerOptions};
use crate::server_manifest::{EnabledTools, ToolContract, ToolManifest};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use toml_edit::Item;

const PROCESS_TIMEOUT: Duration = Duration::from_secs(4);
const MCP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
/// Budget for `initialize`, which also absorbs a cold control-center start.
///
/// The probed server is a thin proxy: it starts the shared control center, waits for it, and only
/// falls back to a standalone server after its own startup timeout. Anything shorter reports a
/// handshake failure for a server that is merely starting.
const MCP_INITIALIZE_TIMEOUT: Duration =
    Duration::from_secs(crate::runtime::STARTUP_TIMEOUT.as_secs() + 5);
static CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Three-state result for one doctor check; Info does not affect the status exit code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoctorCheckStatus {
    /// Configured and passing.
    Pass,
    /// Not yet applied or not currently applicable.
    Info,
    /// Existing configuration is damaged, drifted, or unavailable.
    Fail,
}

/// Result of one doctor check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorCheck {
    /// Stable English check name.
    pub name: &'static str,
    /// Pass, information, or failure state.
    pub status: DoctorCheckStatus,
    /// Current observation.
    pub detail: String,
    /// Recovery step after failure; empty for PASS and INFO.
    pub remedy: Option<String>,
}

impl DoctorCheck {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorCheckStatus::Pass,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn info(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorCheckStatus::Info,
            detail: detail.into(),
            remedy: None,
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            name,
            status: DoctorCheckStatus::Fail,
            detail: detail.into(),
            remedy: Some(remedy.into()),
        }
    }
}

/// Ordered report of all doctor checks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DoctorReport {
    /// Checks in contract-table order.
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Whether no check failed; INFO does not affect the exit code.
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != DoctorCheckStatus::Fail)
    }
}

/// Runs the complete status contract against the configured paths.
pub fn run(paths: &ControlPaths) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(check_profile(paths));

    let config_bytes = match std::fs::read(&paths.codex_config) {
        Ok(bytes) => {
            let check = match std::str::from_utf8(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|source| {
                    toml_edit::DocumentMut::from_str(source)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }) {
                Ok(()) => DoctorCheck::pass(
                    "Codex config",
                    format!("Parsed {}", crate::paths::display_path(&paths.codex_config)),
                ),
                Err(error) => DoctorCheck::fail(
                    "Codex config",
                    format!(
                        "Cannot parse {}: {error}",
                        crate::paths::display_path(&paths.codex_config)
                    ),
                    "Repair config.toml manually, then run fastctx status again.",
                ),
            };
            checks.push(check);
            Some(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            checks.push(DoctorCheck::info(
                "Codex config",
                format!(
                    "{} does not exist yet; Apply will create it.",
                    crate::paths::display_path(&paths.codex_config)
                ),
            ));
            None
        }
        Err(error) => {
            checks.push(DoctorCheck::fail(
                "Codex config",
                format!(
                    "Cannot read {}: {error}",
                    crate::paths::display_path(&paths.codex_config)
                ),
                "Fix the path or permissions, then run fastctx status again.",
            ));
            None
        }
    };

    let settings = settings::load(paths);
    let saved_settings = settings.as_ref().ok();
    let all_applied = saved_settings.and_then(|settings| settings.applied.as_ref());
    let profile_applied = all_applied.filter(|record| record.targets_codex_profile(paths));
    let provider_detection = provider::detect_path(&paths.codex_config);
    checks.push(check_output_guard(
        &provider_detection,
        saved_settings,
        profile_applied,
    ));
    checks.push(check_context_override_math(
        &provider_detection,
        config_bytes.as_deref(),
    ));
    checks.push(check_config_rewrite_detection(
        config_bytes.as_deref(),
        profile_applied,
    ));
    checks.push(check_relay_protocol_self_check(&provider_detection));
    checks.push(match settings.as_ref() {
        Ok(settings) => check_drift(
            paths,
            Some(settings),
            profile_applied,
            config_bytes.as_deref(),
        ),
        Err(error) => DoctorCheck::fail(
            "Applied state",
            error.clone(),
            "Repair ~/.fastctx/config.toml or re-run Apply after moving the damaged file aside.",
        ),
    });
    checks.push(check_binary(paths, all_applied));
    checks.push(check_running_instances(paths));
    let mcp_contract = check_mcp(&paths.installed_binary, profile_applied);
    checks.push(mcp_contract.clone());
    checks.push(check_model_tool_surface(&mcp_contract));
    checks.push(check_agents(paths, profile_applied));
    let fastshell_desired = saved_settings.is_some_and(|settings| settings.fastshell.enabled);
    let fastshell_applied = profile_applied.is_some_and(|record| record.fastshell_enabled);
    checks.push(check_extension_state(
        "fastshell",
        fastshell_desired,
        fastshell_applied,
    ));
    if settings.is_ok() {
        checks.push(check_job_limits(paths));
    }
    checks.push(check_search_parallelism(paths));
    checks.push(DoctorCheck::info(
        "Last update check",
        crate::update::last_check_status(paths).detail,
    ));
    DoctorReport { checks }
}

/// Runs the shared/Codex report and appends scoped checks for every other connected target.
pub fn run_with_connected_targets(paths: &ControlPaths) -> DoctorReport {
    let mut report = run(paths);
    let Ok(settings) = settings::load(paths) else {
        return report;
    };
    for target in crate::control::targets::AgentTarget::ALL {
        if target == crate::control::targets::AgentTarget::Codex
            || settings.target_receipt(target).is_none()
        {
            continue;
        }
        let mut target_report = run_target(paths, target);
        for check in &mut target_report.checks {
            check.detail = format!("{}: {}", target.display_name(), check.detail);
            if let Some(remedy) = &mut check.remedy {
                *remedy = format!("{}: {remedy}", target.display_name());
            }
        }
        report.checks.extend(target_report.checks);
    }
    report
}

/// Runs checks scoped to one agent connection without treating an unapplied target as a global failure.
pub fn run_target(
    paths: &ControlPaths,
    target: crate::control::targets::AgentTarget,
) -> DoctorReport {
    let settings = match settings::load(paths) {
        Ok(settings) => settings,
        Err(error) => {
            return DoctorReport {
                checks: vec![DoctorCheck::fail(
                    "Target settings",
                    error,
                    "Repair ~/.fastctx/config.toml, then retry target Doctor.",
                )],
            };
        }
    };
    let status = crate::control::target_status::inspect_target(paths, &settings, target);
    let mut checks = Vec::new();
    use crate::control::target_status::TargetConnectionState;
    checks.push(match status.state {
        TargetConnectionState::NotConnected => DoctorCheck::info(
            "Target connection",
            format!(
                "{} is not connected. Run fastctx apply --target {}.",
                target.display_name(),
                target.id()
            ),
        ),
        TargetConnectionState::Connected => DoctorCheck::pass(
            "Target connection",
            format!(
                "{} config, guidance, enabled set, ownership receipt, and installed binary agree.",
                target.display_name()
            ),
        ),
        TargetConnectionState::NeedsAttention => DoctorCheck::fail(
            "Target connection",
            status.facts.join(" "),
            format!(
                "Review the drift, then run fastctx apply --target {} to rebuild it or fastctx unapply --target {} to Disconnect safely.",
                target.id(),
                target.id()
            ),
        ),
        TargetConnectionState::PermissionDenied => DoctorCheck::fail(
            "Target connection",
            status.facts.join(" "),
            "Restore read/write permission to the target config and guidance paths, then retry.",
        ),
        TargetConnectionState::Error => DoctorCheck::fail(
            "Target connection",
            status.facts.join(" "),
            "Repair the target configuration format or path, then retry target Doctor.",
        ),
    });
    checks.push(target_budget_check(&settings, target));
    if status.state == TargetConnectionState::Connected {
        checks.push(match probe_mcp(
            &paths.installed_binary,
            ServerOptions::local(status.enabled_tools),
        ) {
            Ok(()) => DoctorCheck::pass(
                "Target MCP contract",
                format!(
                    "The target's {} published tools match their schemas and permission annotations.",
                    status.enabled_tools.names().len()
                ),
            ),
            Err(error) => DoctorCheck::fail(
                "Target MCP contract",
                error,
                "Re-run Apply for this target, then retry target Doctor.",
            ),
        });
    }
    DoctorReport { checks }
}

fn target_budget_check(
    settings: &settings::FastCtxSettings,
    target: crate::control::targets::AgentTarget,
) -> DoctorCheck {
    use crate::control::targets::BudgetPolicy;
    let Some(receipt) = settings.target_receipt(target) else {
        return DoctorCheck::info(
            "Target output budget",
            format!(
                "{}; no applied target budget exists yet.",
                target.budget_policy().doctor_fact()
            ),
        );
    };
    match target.budget_policy() {
        BudgetPolicy::CodexManaged => {
            let Some(codex) = receipt.codex.as_ref() else {
                return DoctorCheck::fail(
                    "Target output budget",
                    "The Codex receipt is missing its host token limit.",
                    "Run fastctx apply --target codex to rebuild the receipt.",
                );
            };
            let envelope = usize::try_from(codex.tool_output_token_limit)
                .unwrap_or_default()
                .saturating_mul(12)
                / 10;
            let per_tool_ok = [
                ("inspect_local_file", codex.tool_budgets.read),
                ("grep", codex.tool_budgets.grep),
                ("glob", codex.tool_budgets.glob),
                ("run", codex.tool_budgets.run),
                ("job_output", codex.tool_budgets.job_output),
            ]
            .into_iter()
            .filter(|(tool, _)| receipt.enabled_tools.contains(tool))
            .all(|(_, budget)| {
                budget.ceiling(receipt.fastctx_token_budget) <= receipt.fastctx_token_budget
            });
            if receipt.fastctx_token_budget <= envelope && per_tool_ok {
                DoctorCheck::pass(
                    "Target output budget",
                    format!(
                        "FastCtx budget {} <= Codex host envelope {} × 1.2 = {envelope}; every per-tool budget is within the FastCtx budget.",
                        receipt.fastctx_token_budget, codex.tool_output_token_limit
                    ),
                )
            } else {
                DoctorCheck::fail(
                    "Target output budget",
                    "Codex host/global/per-tool output budgets violate the managed envelope.",
                    "Run fastctx apply --target codex to realign the budgets.",
                )
            }
        }
        BudgetPolicy::ClaudeDocumented
        | BudgetPolicy::OpenCodeByteCeiling
        | BudgetPolicy::UnknownHost => DoctorCheck::info(
            "Target output budget",
            format!(
                "FastCtx uses {} tokens; {}.",
                receipt.fastctx_token_budget,
                target.budget_policy().doctor_fact()
            ),
        ),
    }
}

fn check_running_instances(paths: &ControlPaths) -> DoctorCheck {
    match crate::control::processes::installed_processes(&paths.fastctx_bin_dir) {
        Ok(processes) => {
            let pids = processes
                .iter()
                .filter(|process| process.identity.pid != std::process::id())
                .map(|process| process.identity.pid.to_string())
                .collect::<Vec<_>>();
            if pids.is_empty() {
                DoctorCheck::info(
                    "Running server instances",
                    "No other FastCtx process images are running from the managed bin directory.",
                )
            } else {
                DoctorCheck::pass(
                    "Running server instances",
                    format!(
                        "{} managed FastCtx process image(s) are running; PID(s): {}. This is informational and does not classify session health.",
                        pids.len(),
                        pids.join(", ")
                    ),
                )
            }
        }
        Err(error) => DoctorCheck::info(
            "Running server instances",
            format!("Running FastCtx process images could not be enumerated: {error}"),
        ),
    }
}

fn check_job_limits(paths: &ControlPaths) -> DoctorCheck {
    match settings::job_limit_status(paths) {
        Ok(status) => {
            let effective = format!(
                "Effective current-user limits: {} MiB retained job storage; {} running jobs; {} records per job_list page.",
                status.job_storage_limit_mib, status.max_running_jobs, status.job_list_limit
            );
            let mut invalid = Vec::new();
            if status.storage_limit_fell_back {
                invalid.push("fastshell.job_storage_limit_mib");
            }
            if status.running_limit_fell_back {
                invalid.push("fastshell.max_running_jobs");
            }
            if status.list_limit_fell_back {
                invalid.push("fastshell.job_list_limit");
            }
            if invalid.is_empty() {
                DoctorCheck::pass("Job limits", effective)
            } else {
                DoctorCheck::info(
                    "Job limits",
                    format!(
                        "Invalid {} value(s) fell back to safe defaults. {effective}",
                        invalid.join(", ")
                    ),
                )
            }
        }
        Err(error) => DoctorCheck::fail(
            "Job limits",
            error,
            "Repair ~/.fastctx/config.toml, then run fastctx status again.",
        ),
    }
}

fn check_search_parallelism(paths: &ControlPaths) -> DoctorCheck {
    match settings::search_parallelism_status(paths) {
        Ok(status) => match (status.configured, status.effective) {
            (None, Some(effective)) => DoctorCheck::pass(
                "Search CPU limit",
                format!(
                    "Automatic grep/glob parallelism: engine-visible upper bound {0}; effective P={effective} for the next control center. No user CPU limit is configured. A running control center keeps the parallelism it started with until it exits, which happens once every Codex application using it has quit.",
                    status.available
                ),
            ),
            (Some(configured), Some(effective)) => DoctorCheck::pass(
                "Search CPU limit",
                format!(
                    "Configured search.max_cpu_cores={configured}; engine-visible upper bound {}; effective P={effective} for the next control center. A running control center keeps the parallelism it started with until it exits, which happens once every Codex application using it has quit.",
                    status.available
                ),
            ),
            (Some(configured), None) => DoctorCheck::fail(
                "Search CPU limit",
                format!(
                    "search.max_cpu_cores={configured} is invalid on this machine; the legal range is 1..={}.",
                    status.available
                ),
                "Set search.max_cpu_cores to a legal integer or remove the key for automatic parallelism. The next control center picks up the value once every Codex application using the running one has quit.",
            ),
            (None, None) => unreachable!("automatic parallelism always resolves"),
        },
        Err(error) => DoctorCheck::fail(
            "Search CPU limit",
            error,
            "Repair ~/.fastctx/config.toml, then run fastctx status again.",
        ),
    }
}

fn check_profile(paths: &ControlPaths) -> DoctorCheck {
    let profile = crate::paths::display_path(&paths.codex_dir);
    let source = paths.codex_home_source.as_str();
    match std::fs::metadata(&paths.codex_dir) {
        Ok(metadata) if metadata.is_dir() => DoctorCheck::pass(
            "Codex profile",
            format!("Configuration root: {profile} (source: {source})."),
        ),
        Ok(_) => DoctorCheck::fail(
            "Codex profile",
            format!(
                "Configuration root {profile} (source: {source}) exists but is not a directory."
            ),
            "Move or remove that path so Apply can create the configuration directory.",
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DoctorCheck::info(
            "Codex profile",
            format!(
                "Configuration root {profile} (source: {source}) does not exist yet; Apply will create it."
            ),
        ),
        Err(error) => DoctorCheck::fail(
            "Codex profile",
            format!("Cannot inspect configuration root {profile} (source: {source}): {error}"),
            "Fix the path or permissions, then run fastctx status again.",
        ),
    }
}

fn check_output_guard(
    detection: &provider::ProviderDetection,
    settings: Option<&settings::FastCtxSettings>,
    record: Option<&settings::AppliedRecord>,
) -> DoctorCheck {
    if detection.provenance == ProviderProvenance::Unknown {
        let settings_detail = if settings.is_none() {
            " FastCtx settings also could not be loaded, so no effective output policy could be evaluated."
        } else {
            ""
        };
        return DoctorCheck::info(
            "Provider and compaction",
            format!(
                "WARNING: {} FastCtx never tightens an unresolved provider silently, so Guarded was not activated.{settings_detail} Repair or verify model_provider, then run fastctx status again.",
                detection.detail
            ),
        );
    }
    let Some(settings) = settings else {
        return DoctorCheck::info(
            "Provider and compaction",
            format!(
                "{} FastCtx settings could not be loaded, so the effective output policy could not be evaluated.",
                detection.detail
            ),
        );
    };
    match detection.provenance {
        ProviderProvenance::Unknown => unreachable!("unknown providers returned above"),
        ProviderProvenance::LocalRuntime | ProviderProvenance::ThirdPartyRelay
            if !settings.output_guard.enabled =>
        {
            DoctorCheck::info(
                "Provider and compaction",
                format!(
                    "{} Guarded protection is explicitly disabled. A large FastCtx turn can cross the remaining compaction margin, and FastCtx cannot make the provider's catalog, usage, compaction, or context-error contract correct.",
                    detection.detail
                ),
            )
        }
        ProviderProvenance::LocalRuntime | ProviderProvenance::ThirdPartyRelay => {
            let effective =
                provider::effective_output(settings.tier, settings.tool_budgets, true, detection);
            match record {
                Some(record)
                    if record.tool_output_token_limit == effective.host_limit
                        && record.fastctx_token_budget == effective.fastctx_budget
                        && record.tool_budgets == effective.tool_budgets =>
                {
                    DoctorCheck::pass(
                        "Provider and compaction",
                        format!(
                            "{} Guarded is active: host limit {}, FastCtx per-call budget {}, within Codex 0.151.0's host-limit × 1.2 tool-output envelope. This prevents host middle truncation for FastCtx output but cannot verify the other relay contracts.",
                            detection.detail, effective.host_limit, effective.fastctx_budget
                        ),
                    )
                }
                Some(record) => DoctorCheck::fail(
                    "Provider and compaction",
                    format!(
                        "{} New runtime sessions are constrained to Guarded, but the Apply receipt still records host/global limits {}/{} instead of {}/{}.",
                        detection.detail,
                        record.tool_output_token_limit,
                        record.fastctx_token_budget,
                        effective.host_limit,
                        effective.fastctx_budget
                    ),
                    "Run fastctx apply to preview and write the Guarded host and server limits into Codex.",
                ),
                None => DoctorCheck::info(
                    "Provider and compaction",
                    format!(
                        "{} New runtime sessions use Guarded automatically; run fastctx apply to write host limit {} and FastCtx budget {} into Codex.",
                        detection.detail, effective.host_limit, effective.fastctx_budget
                    ),
                ),
            }
        }
        ProviderProvenance::OfficialOpenAi
        | ProviderProvenance::Azure
        | ProviderProvenance::AmazonBedrock => {
            let effective = provider::effective_output(
                settings.tier,
                settings.tool_budgets,
                settings.output_guard.enabled,
                detection,
            );
            match record {
                Some(record)
                    if record.tool_output_token_limit == effective.host_limit
                        && record.fastctx_token_budget == effective.fastctx_budget
                        && record.tool_budgets == effective.tool_budgets =>
                {
                    DoctorCheck::pass("Provider and compaction", detection.detail.clone())
                }
                Some(_) => DoctorCheck::info(
                    "Provider and compaction",
                    format!(
                        "{} Guarded is no longer required. Run fastctx apply to restore the selected {} tier (host limit {}, FastCtx budget {}).",
                        detection.detail,
                        settings.tier.display_name(),
                        effective.host_limit,
                        effective.fastctx_budget
                    ),
                ),
                None => DoctorCheck::info(
                    "Provider and compaction",
                    format!(
                        "{} FastCtx has not been applied in this profile.",
                        detection.detail
                    ),
                ),
            }
        }
    }
}

fn check_context_override_math(
    detection: &provider::ProviderDetection,
    config: Option<&[u8]>,
) -> DoctorCheck {
    let Some(config) = config else {
        return DoctorCheck::info(
            "Context override math",
            "No readable Codex config is available, so model_context_window and model_auto_compact_token_limit could not be inspected.",
        );
    };
    let document = match std::str::from_utf8(config)
        .map_err(|error| error.to_string())
        .and_then(|source| {
            toml_edit::DocumentMut::from_str(source).map_err(|error| error.to_string())
        }) {
        Ok(document) => document,
        Err(error) => {
            return DoctorCheck::info(
                "Context override math",
                format!("Codex config could not be evaluated for context overrides: {error}"),
            );
        }
    };
    let context = positive_override(&document, "model_context_window");
    let auto = positive_override(&document, "model_auto_compact_token_limit");
    for (key, value) in [
        ("model_context_window", &context),
        ("model_auto_compact_token_limit", &auto),
    ] {
        if matches!(value, OverrideValue::Invalid) {
            return DoctorCheck::fail(
                "Context override math",
                format!("Codex config key {key} is present but is not a positive integer."),
                format!(
                    "Remove or repair {key}, then use Codex /status to verify the resolved model limits."
                ),
            );
        }
    }
    let context = context.value();
    let auto = auto.value();
    if context.is_none() && auto.is_none() {
        return DoctorCheck::pass(
            "Context override math",
            "No model_context_window or model_auto_compact_token_limit override is configured; Codex resolves both from its selected model catalog. Confirm the resolved values with Codex /status.",
        );
    }

    let context_term = context.map_or_else(
        || "catalog max_context_window".to_string(),
        |value| format!("min({value}, catalog max_context_window)"),
    );
    let auto_term = auto.map_or_else(
        || "floor(90% × resolved window)".to_string(),
        |value| format!("min({value}, floor(90% × resolved window))"),
    );
    let keys = [
        context.map(|value| format!("model_context_window={value}")),
        auto.map(|value| format!("model_auto_compact_token_limit={value}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" and ");
    let scenarios = [272_000_i64, 1_000_000_i64]
        .map(|catalog_max| {
            let resolved_window = context
                .unwrap_or(catalog_max)
                .min(catalog_max);
            let ninety_percent = resolved_window.saturating_mul(9) / 10;
            let resolved_auto = auto.unwrap_or(ninety_percent).min(ninety_percent);
            format!(
                "catalog max {catalog_max} => window {resolved_window}, auto-compact {resolved_auto}"
            )
        })
        .join("; ");
    let relay_warning = if detection.provenance == ProviderProvenance::ThirdPartyRelay {
        " This is a third-party route: a high catalog can make a high override effective even when the real backend wall is lower."
    } else {
        ""
    };
    DoctorCheck::info(
        "Context override math",
        format!(
            "Codex config sets {keys}. Under Codex 0.147.0 rules, resolved window = {context_term}; resolved auto-compact limit = {auto_term}. Applying those exact configured numbers to two illustrative catalog maxima: {scenarios}.{relay_warning} FastCtx does not mirror the live model catalog: use Codex /status to read the resolved values, and remove the overrides unless their catalog provenance is reliable."
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverrideValue {
    Missing,
    Positive(i64),
    Invalid,
}

impl OverrideValue {
    const fn value(self) -> Option<i64> {
        match self {
            Self::Positive(value) => Some(value),
            Self::Missing | Self::Invalid => None,
        }
    }
}

fn positive_override(document: &toml_edit::DocumentMut, key: &str) -> OverrideValue {
    match document.get(key) {
        None => OverrideValue::Missing,
        Some(item) => match item.as_integer().filter(|value| *value > 0) {
            Some(value) => OverrideValue::Positive(value),
            None => OverrideValue::Invalid,
        },
    }
}

fn check_config_rewrite_detection(
    config: Option<&[u8]>,
    record: Option<&settings::AppliedRecord>,
) -> DoctorCheck {
    let Some(record) = record else {
        return DoctorCheck::info(
            "Config rewrite detection",
            "No Apply receipt owns FastCtx keys in this Codex profile, so whole-file rewrite detection is not applicable yet.",
        );
    };
    let Some(config) = config else {
        return DoctorCheck::fail(
            "Config rewrite detection",
            "An Apply receipt owns FastCtx keys, but Codex config.toml is unavailable.",
            "Restore a readable config.toml, then run fastctx apply to recreate only the managed entries.",
        );
    };
    let source = match std::str::from_utf8(config) {
        Ok(source) => source,
        Err(error) => {
            return DoctorCheck::info(
                "Config rewrite detection",
                format!(
                    "Codex config is not UTF-8, so managed-key presence could not be checked: {error}"
                ),
            );
        }
    };
    let document = match toml_edit::DocumentMut::from_str(source) {
        Ok(document) => document,
        Err(error) => {
            return DoctorCheck::info(
                "Config rewrite detection",
                format!("Codex config could not be parsed for managed-key presence: {error}"),
            );
        }
    };
    let missing = missing_receipt_keys(&document, record);
    if missing.is_empty() {
        DoctorCheck::pass(
            "Config rewrite detection",
            "The Apply receipt and Codex config still share every FastCtx-managed key. Host-owned edits and formatting changes are intentionally ignored; semantic value drift is checked separately.",
        )
    } else {
        DoctorCheck::fail(
            "Config rewrite detection",
            format!(
                "The Apply receipt records FastCtx-managed keys that disappeared from config.toml: {}. A config switcher or other tool may have rewritten the whole file from an older snapshot.",
                missing.join(", ")
            ),
            "Run fastctx apply to recreate only the managed entries, then configure the other tool to preserve current config.toml keys.",
        )
    }
}

fn missing_receipt_keys(
    document: &toml_edit::DocumentMut,
    record: &settings::AppliedRecord,
) -> Vec<&'static str> {
    let mut missing = Vec::new();
    if document.get("tool_output_token_limit").is_none() {
        missing.push("tool_output_token_limit");
    }
    let fastctx = document
        .get("mcp_servers")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("fastctx"))
        .and_then(Item::as_table_like);
    let Some(fastctx) = fastctx else {
        missing.push("mcp_servers.fastctx");
        return missing;
    };
    for key in ["command", "args", "startup_timeout_sec"] {
        if fastctx.get(key).is_none() {
            missing.push(match key {
                "command" => "mcp_servers.fastctx.command",
                "args" => "mcp_servers.fastctx.args",
                _ => "mcp_servers.fastctx.startup_timeout_sec",
            });
        }
    }
    if record.tool_timeout_sec.is_some() && fastctx.get("tool_timeout_sec").is_none() {
        missing.push("mcp_servers.fastctx.tool_timeout_sec");
    }
    let env = fastctx.get("env").and_then(Item::as_table_like);
    let Some(env) = env else {
        missing.push("mcp_servers.fastctx.env");
        return missing;
    };
    if env.get("FASTCTX_TOKEN_BUDGET").is_none() {
        missing.push("mcp_servers.fastctx.env.FASTCTX_TOKEN_BUDGET");
    }
    let tools = applied_tools(record);
    for (tool, key, budget) in [
        (
            "inspect_local_file",
            "FASTCTX_READ_TOKEN_BUDGET",
            record.tool_budgets.read,
        ),
        (
            "grep",
            "FASTCTX_GREP_TOKEN_BUDGET",
            record.tool_budgets.grep,
        ),
        (
            "glob",
            "FASTCTX_GLOB_TOKEN_BUDGET",
            record.tool_budgets.glob,
        ),
        ("run", "FASTCTX_RUN_TOKEN_BUDGET", record.tool_budgets.run),
        (
            "job_output",
            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
            record.tool_budgets.job_output,
        ),
    ] {
        if tools.contains(tool)
            && budget.resolve(record.fastctx_token_budget).is_some()
            && env.get(key).is_none()
        {
            missing.push(match key {
                "FASTCTX_READ_TOKEN_BUDGET" => "mcp_servers.fastctx.env.FASTCTX_READ_TOKEN_BUDGET",
                "FASTCTX_GREP_TOKEN_BUDGET" => "mcp_servers.fastctx.env.FASTCTX_GREP_TOKEN_BUDGET",
                "FASTCTX_GLOB_TOKEN_BUDGET" => "mcp_servers.fastctx.env.FASTCTX_GLOB_TOKEN_BUDGET",
                "FASTCTX_RUN_TOKEN_BUDGET" => "mcp_servers.fastctx.env.FASTCTX_RUN_TOKEN_BUDGET",
                _ => "mcp_servers.fastctx.env.FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
            });
        }
    }
    missing
}

fn check_relay_protocol_self_check(detection: &provider::ProviderDetection) -> DoctorCheck {
    if detection.provenance != ProviderProvenance::ThirdPartyRelay {
        return DoctorCheck::info(
            "Relay protocol self-check",
            "No third-party relay is selected. FastCtx does not observe model usage or upstream context-error payloads, so this check remains informational.",
        );
    }
    let compact = match detection.codex_compaction {
        CodexCompaction::RemoteV2 => {
            "Codex will send V2 compaction to this relay; failure has no local fallback."
        }
        CodexCompaction::Local => {
            "Codex will compact locally, but the relay still owns catalog, usage, and error fidelity."
        }
        CodexCompaction::RemoteV1 | CodexCompaction::Unknown => {
            "The relay compaction path is unresolved."
        }
    };
    DoctorCheck::info(
        "Relay protocol self-check",
        format!(
            "{compact} FastCtx cannot test the relay from inside MCP. In a long Codex session, watch /status: if used tokens barely move after substantial responses, the relay may be dropping terminal usage. Also verify that context failures preserve code=context_length_exceeded; a generic 400/500 does not enter Codex's context-specific recovery path."
        ),
    )
}

fn check_drift(
    paths: &ControlPaths,
    settings: Option<&settings::FastCtxSettings>,
    profile_applied: Option<&settings::AppliedRecord>,
    config: Option<&[u8]>,
) -> DoctorCheck {
    let Some(record) = profile_applied else {
        if let Some(record) = settings.and_then(|settings| settings.applied.as_ref()) {
            let recorded_profile = Path::new(&record.codex_config.path)
                .parent()
                .unwrap_or_else(|| Path::new(&record.codex_config.path));
            return DoctorCheck::info(
                "Applied state",
                format!(
                    "The saved Apply receipt targets {}; fastctx has not been applied in the selected profile. Run fastctx apply when ready.",
                    crate::paths::display_path(recorded_profile)
                ),
            );
        }
        return DoctorCheck::info(
            "Applied state",
            "fastctx has not been applied in this profile. Run fastctx apply when ready.",
        );
    };
    let Some(config) = config else {
        return DoctorCheck::fail(
            "Applied state",
            "Codex config could not be inspected.",
            "Repair Codex config.toml, then re-apply.",
        );
    };
    let expected = ExpectedConfig {
        command: record.command.clone(),
        tier: record.tier,
        host_limit: record.tool_output_token_limit,
        fastctx_budget: record.fastctx_token_budget,
        tool_budgets: record.tool_budgets,
        enabled_tools: applied_tools(record),
    };
    let legacy_fastedit =
        settings.is_some_and(|settings| settings.fastedit.enabled) || record.fastedit_enabled;
    match codex_config::drift_applied(
        config,
        &expected,
        record.tool_output_token_limit,
        record.fastctx_token_budget,
        record.tool_timeout_sec,
    )
    .and_then(|mut items| {
        items.extend(receipt_drift(paths, record)?);
        if legacy_fastedit {
            items.push("legacy fastedit configuration".to_string());
        }
        items.sort();
        items.dedup();
        Ok(items)
    }) {
        Ok(items) if items.is_empty() => {
            let mut pending = Vec::new();
            if let Some(settings) = settings {
                let detection = provider::detect_bytes(Some(config));
                let effective = provider::effective_output(
                    settings.tier,
                    settings.tool_budgets,
                    settings.output_guard.enabled,
                    &detection,
                );
                if record.tool_output_token_limit != effective.host_limit
                    || record.fastctx_token_budget != effective.fastctx_budget
                {
                    pending.push(match effective.mode {
                        EffectiveOutputMode::SelectedTier => "the current tier limits",
                        EffectiveOutputMode::Guarded => "the current provider's Guarded limits",
                    });
                }
                if settings.tier != record.tier {
                    pending.push("the selected tier");
                }
                // Compare resolved shares, not stored preferences: an unset entry means "follow
                // the tier", so it only differs from the receipt once the tier's defaults do.
                if effective.tool_budgets != record.tool_budgets {
                    pending.push("the per-tool output budgets");
                }
            }
            if pending.is_empty() {
                DoctorCheck::pass(
                    "Applied state",
                    "Managed Codex settings match the Apply receipt.",
                )
            } else {
                DoctorCheck::info(
                    "Applied state",
                    format!(
                        "Managed Codex settings still match the previous Apply receipt, but Apply is pending for {}. Run fastctx apply to preview and write the current output settings into Codex.",
                        pending.join(" and ")
                    ),
                )
            }
        }
        Ok(items) => DoctorCheck::fail(
            "Applied state",
            format!("Drift detected: {}", items.join(", ")),
            "Run fastctx apply to preview and repair only the managed entries.",
        ),
        Err(error) => DoctorCheck::fail(
            "Applied state",
            error,
            "Repair Codex config.toml, then re-apply.",
        ),
    }
}

fn check_binary(paths: &ControlPaths, record: Option<&settings::AppliedRecord>) -> DoctorCheck {
    if paths.installed_binary.exists() && !paths.installed_binary.is_file() {
        return DoctorCheck::fail(
            "Installed binary",
            format!(
                "{} exists but is not a regular file.",
                crate::paths::display_path(&paths.installed_binary)
            ),
            "Move or remove that path, then run fastctx apply.",
        );
    }
    if !paths.installed_binary.is_file() {
        if record.is_none() {
            return DoctorCheck::info(
                "Installed binary",
                format!(
                    "{} is not installed yet; Apply will create it.",
                    crate::paths::display_path(&paths.installed_binary)
                ),
            );
        }
        return DoctorCheck::fail(
            "Installed binary",
            format!(
                "{} is missing.",
                crate::paths::display_path(&paths.installed_binary)
            ),
            "Run fastctx apply to install the stable binary.",
        );
    }
    if let Some(record) = record {
        match std::fs::read(&paths.installed_binary) {
            Ok(bytes) if sha256(&bytes) != record.binary_sha256 => {
                return DoctorCheck::fail(
                    "Installed binary",
                    "The installed binary content does not match the Apply receipt.",
                    "Run fastctx apply to refresh the stable binary.",
                );
            }
            Ok(_) => {}
            Err(error) => {
                return DoctorCheck::fail(
                    "Installed binary",
                    format!(
                        "Cannot read {}: {error}",
                        crate::paths::display_path(&paths.installed_binary)
                    ),
                    "Run fastctx apply to replace the stable binary.",
                );
            }
        }
    }
    let result = match run_output(
        crate::process_policy::noninteractive_command(&paths.installed_binary).arg("--version"),
        PROCESS_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => {
            let actual = output_detail(&output);
            let expected = format!("fastctx {}", env!("CARGO_PKG_VERSION"));
            if actual == expected {
                DoctorCheck::pass("Installed binary", actual)
            } else {
                DoctorCheck::fail(
                    "Installed binary",
                    format!("Expected {expected}, got {actual}."),
                    "Run fastctx apply to refresh the stable binary.",
                )
            }
        }
        Ok(output) => DoctorCheck::fail(
            "Installed binary",
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            "Run fastctx apply to replace the stable binary.",
        ),
        Err(error) => DoctorCheck::fail(
            "Installed binary",
            error,
            "Run fastctx apply to replace the stable binary.",
        ),
    };
    if record.is_none() && result.status == DoctorCheckStatus::Pass {
        DoctorCheck::info(
            "Installed binary",
            format!(
                "{} is runnable but is not owned by an Apply receipt; Apply will adopt and record it.",
                crate::paths::display_path(&paths.installed_binary)
            ),
        )
    } else {
        result
    }
}

fn receipt_drift(
    paths: &ControlPaths,
    record: &settings::AppliedRecord,
) -> Result<Vec<String>, String> {
    let mut drift = Vec::new();
    // Codex rewrites unowned config fields on startup, and users may edit outside our AGENTS block.
    // Status therefore validates those managed semantics separately instead of hashing whole files (2026-07-17).
    check_recorded_path(
        &paths.codex_config,
        &record.codex_config,
        "Codex config receipt",
        &mut drift,
    );
    check_recorded_path(
        &paths.codex_agents,
        &record.codex_agents,
        "AGENTS receipt",
        &mut drift,
    );
    if record.command != crate::paths::display_path(&paths.installed_binary) {
        drift.push("installed binary receipt path".to_string());
    }
    match std::fs::read(&paths.installed_binary) {
        Ok(bytes) if sha256(&bytes) != record.binary_sha256 => {
            drift.push("installed binary content".to_string())
        }
        Ok(_) => {}
        Err(error) => {
            return Err(format!(
                "Cannot read installed binary {} while checking the Apply receipt: {error}",
                crate::paths::display_path(&paths.installed_binary)
            ));
        }
    }
    Ok(drift)
}

fn check_recorded_path(
    path: &Path,
    record: &settings::ManagedFileRecord,
    label: &str,
    drift: &mut Vec<String>,
) {
    if record.path != crate::paths::display_path(path) {
        drift.push(format!("{label} path"));
    }
}

fn check_mcp(executable: &Path, applied: Option<&settings::AppliedRecord>) -> DoctorCheck {
    if !executable.is_file() && applied.is_none() {
        return DoctorCheck::info(
            "MCP server contract",
            "Not run before Apply installs the stable fastctx binary.",
        );
    }
    let options = applied.map_or_else(ServerOptions::default, |record| {
        ServerOptions::local(applied_tools(record))
    });
    match probe_mcp(executable, options) {
        Ok(()) => DoctorCheck::pass(
            "MCP server contract",
            format!(
                "FastCtx initialize and tools/list returned {} tools with matching contract hashes. This proves the server contract only, not model-side tool exposure.",
                ToolManifest::expected_names(options.tools).len()
            ),
        ),
        Err(error) => DoctorCheck::fail(
            "MCP server contract",
            error,
            "Run fastctx apply, then retry status. If it still fails, run the configured fastctx serve command from a terminal to inspect the error.",
        ),
    }
}

fn check_model_tool_surface(server_contract: &DoctorCheck) -> DoctorCheck {
    let detail = match server_contract.status {
        DoctorCheckStatus::Pass => {
            "Unverified: FastCtx can validate its own MCP server contract, but cannot observe whether Codex or the configured provider exposed those tools to the model. Start a new Codex session and verify one direct FastCtx tool call."
        }
        DoctorCheckStatus::Info => {
            "Unverified: the MCP server contract was not probed, and FastCtx cannot observe model-side tool exposure. Run fastctx apply, start a new Codex session, and verify one direct FastCtx tool call."
        }
        DoctorCheckStatus::Fail => {
            "Unverified: the MCP server contract failed, and even a passing server contract would not prove model-side tool exposure. Repair the server contract, start a new Codex session, and verify one direct FastCtx tool call."
        }
    };
    DoctorCheck::info("Model tool surface", detail)
}

fn check_extension_state(name: &'static str, desired: bool, applied: bool) -> DoctorCheck {
    match (desired, applied) {
        (false, false) => DoctorCheck::info(
            name,
            format!("{name} is disabled. Enable it in Config and run Apply to register it."),
        ),
        (true, false) => DoctorCheck::info(
            name,
            format!("{name} is enabled in Config and will be registered by the next Apply."),
        ),
        (false, true) => DoctorCheck::info(
            name,
            format!(
                "{name} is still applied but is disabled in Config; the next Apply will remove it."
            ),
        ),
        (true, true) => DoctorCheck::pass(name, format!("{name} is enabled and applied.")),
    }
}

fn check_agents(paths: &ControlPaths, applied: Option<&settings::AppliedRecord>) -> DoctorCheck {
    match std::fs::read(&paths.codex_agents) {
        Ok(bytes) => {
            let state = applied.map_or_else(
                || classify_unowned_agents(&bytes),
                |record| agents::classify_managed_section_for_tools(&bytes, applied_tools(record)),
            );
            check_agents_state(paths, applied, state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            check_agents_state(paths, applied, agents::ManagedSectionState::Missing)
        }
        Err(error) => DoctorCheck::fail(
            "AGENTS guidance",
            format!(
                "State: unreadable. Cannot read {}: {error}",
                crate::paths::display_path(&paths.codex_agents)
            ),
            "Fix the path or permissions, run fastctx apply, then restart Codex.",
        ),
    }
}

fn classify_unowned_agents(bytes: &[u8]) -> agents::ManagedSectionState {
    let file_names = ["inspect_local_file", "grep", "glob", "replace"];
    let shell_names = [
        "run",
        "run_background",
        "job_output",
        "job_kill",
        "job_list",
    ];
    for mask in 1_u8..16 {
        for shell in [false, true] {
            let names = file_names
                .iter()
                .enumerate()
                .filter_map(|(index, name)| (mask & (1 << index) != 0).then_some(*name))
                .chain(shell.then_some(shell_names).into_iter().flatten())
                .collect::<Vec<_>>();
            let tools = EnabledTools::from_names(names)
                .expect("the closed Doctor enumeration only builds valid tool sets");
            let state = agents::classify_managed_section_for_tools(bytes, tools);
            if !matches!(state, agents::ManagedSectionState::Drifted) {
                return state;
            }
        }
    }
    agents::ManagedSectionState::Drifted
}

fn applied_tools(record: &settings::AppliedRecord) -> EnabledTools {
    record.enabled_tools.unwrap_or_else(|| {
        if record.fastshell_enabled {
            EnabledTools::all()
        } else {
            EnabledTools::files()
        }
    })
}

fn check_agents_state(
    paths: &ControlPaths,
    applied: Option<&settings::AppliedRecord>,
    state: agents::ManagedSectionState,
) -> DoctorCheck {
    let has_receipt = applied.is_some();
    match state {
        agents::ManagedSectionState::Current
            if applied.is_some_and(|record| {
                record.agents_contract_id.as_deref() == Some(agents::MANAGED_SECTION_CONTRACT_ID)
            }) =>
        {
            DoctorCheck::pass(
                "AGENTS guidance",
                "State: current. The managed block and explicit Apply receipt match the current contract.",
            )
        }
        agents::ManagedSectionState::Current if has_receipt => DoctorCheck::fail(
            "AGENTS guidance",
            "State: current, Apply required. The managed bytes are current, but the receipt does not record this contract; a safe automatic refresh never impersonates a complete Apply.",
            "Run fastctx apply to record the complete connection, then restart Codex.",
        ),
        agents::ManagedSectionState::Current => DoctorCheck::info(
            "AGENTS guidance",
            "State: current without an Apply receipt. Automatic updates will not claim ownership; run fastctx apply to record the connection, then restart Codex.",
        ),
        agents::ManagedSectionState::KnownLegacy
            if applied.is_some_and(|record| record.agents_contract_id.is_some()) =>
        {
            DoctorCheck::fail(
                "AGENTS guidance",
                "State: superseded guidance after Apply. The receipt already records a guidance contract, so these bytes are post-Apply drift and automatic updates preserve them.",
                "Run fastctx apply to replace the managed block and record the complete connection, then restart Codex.",
            )
        }
        agents::ManagedSectionState::KnownLegacy if has_receipt => DoctorCheck::fail(
            "AGENTS guidance",
            "State: superseded guidance. An exact managed block from an earlier release remains. A managed product update may safely refresh this exact block, but that refresh never completes Apply.",
            "Run fastctx apply to replace the managed block and record the complete connection, then restart Codex.",
        ),
        agents::ManagedSectionState::KnownLegacy => DoctorCheck::fail(
            "AGENTS guidance",
            "State: superseded guidance without an Apply receipt. Automatic updates will not rewrite it because FastCtx cannot prove ownership.",
            "Run fastctx apply to adopt and replace the managed block, then restart Codex.",
        ),
        agents::ManagedSectionState::Missing if has_receipt => DoctorCheck::fail(
            "AGENTS guidance",
            format!(
                "State: missing. An Apply receipt exists, but {} has no managed guidance block. Automatic updates never recreate a missing block.",
                crate::paths::display_path(&paths.codex_agents)
            ),
            "Run fastctx apply to recreate only the managed block, then restart Codex.",
        ),
        agents::ManagedSectionState::Missing => DoctorCheck::info(
            "AGENTS guidance",
            format!(
                "State: missing. No Apply receipt owns a block in {}; fastctx apply will add one without changing other content.",
                crate::paths::display_path(&paths.codex_agents)
            ),
        ),
        agents::ManagedSectionState::Drifted if has_receipt => DoctorCheck::fail(
            "AGENTS guidance",
            "State: drifted. The marker pair is valid, but its managed bytes do not match a current or known legacy contract. Automatic updates preserve these changed bytes.",
            "Review the managed block, run fastctx apply to replace it, then restart Codex.",
        ),
        agents::ManagedSectionState::Drifted => DoctorCheck::fail(
            "AGENTS guidance",
            "State: drifted without an Apply receipt. Automatic updates preserve the unowned marker block.",
            "Review the managed block, run fastctx apply to adopt and replace it, then restart Codex.",
        ),
        agents::ManagedSectionState::Malformed(error) => DoctorCheck::fail(
            "AGENTS guidance",
            format!("State: malformed. {error} Automatic updates never rewrite malformed markers."),
            "Repair AGENTS.md as UTF-8 with at most one fastctx marker pair, run fastctx apply, then restart Codex.",
        ),
    }
}

/// Runs MCP initialize and tools/list through a real child process.
pub fn probe_mcp(executable: &Path, options: ServerOptions) -> Result<(), String> {
    let expected = FastCtxServer::with_options(options).tool_contracts();
    let tools = options.tools.names().join(",");
    probe_mcp_server(executable, &["serve", "--tools", &tools], &expected)
}

/// Probes one explicit server invocation and requires exact tool contracts.
pub fn probe_mcp_server(
    executable: &Path,
    arguments: &[&str],
    expected_contracts: &[ToolContract],
) -> Result<(), String> {
    let mut child = crate::process_policy::noninteractive_command(executable);
    child
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = child
        .spawn()
        .map_err(|error| format!("MCP spawn failed: {error}"))?;
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            terminate(&mut child);
            return Err("MCP spawn failed: stdin was not piped.".to_string());
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate(&mut child);
            return Err("MCP spawn failed: stdout was not piped.".to_string());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate(&mut child);
            return Err("MCP spawn failed: stderr was not piped.".to_string());
        }
    };
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    break;
                }
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut stderr = stderr;
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let result = (|| {
        send_json(
            &mut stdin,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "fastctx-doctor", "version": env!("CARGO_PKG_VERSION")}
                }
            }),
        )?;
        let initialized = receive_response(&receiver, 1, "initialize", MCP_INITIALIZE_TIMEOUT)?;
        if initialized.get("error").is_some() {
            return Err(format!(
                "MCP handshake failed during initialize: {initialized}"
            ));
        }
        send_json(
            &mut stdin,
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )?;
        send_json(
            &mut stdin,
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )?;
        let listed = receive_response(&receiver, 2, "tools/list", MCP_RESPONSE_TIMEOUT)?;
        if listed.get("error").is_some() {
            return Err(format!("MCP handshake failed during tools/list: {listed}"));
        }
        let tools = listed["result"]["tools"]
            .as_array()
            .ok_or_else(|| format!("MCP tools/list returned an invalid payload: {listed}"))?;
        let definitions = tools
            .iter()
            .cloned()
            .map(serde_json::from_value::<rmcp::model::Tool>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!("MCP tools/list returned an invalid tool definition: {error}")
            })?;
        let enabled = EnabledTools::from_names(
            expected_contracts
                .iter()
                .map(|contract| contract.name.as_str()),
        )?;
        ToolManifest::validate(&definitions, enabled)
            .map_err(|error| format!("MCP tools/list manifest mismatch: {error}"))?;
        let actual = ToolManifest::contracts(&definitions)?
            .into_iter()
            .map(|contract| (contract.name, contract.hash))
            .collect::<BTreeMap<_, _>>();
        let expected = expected_contracts
            .iter()
            .map(|contract| (contract.name.clone(), contract.hash.clone()))
            .collect::<BTreeMap<_, _>>();
        if actual != expected {
            return Err(format!(
                "MCP tools/list contract hashes differ: expected {expected:?}, got {actual:?}."
            ));
        }
        Ok(())
    })();

    drop(stdin);
    if result.is_err() {
        terminate(&mut child);
    }
    let exit = wait_child(&mut child, PROCESS_TIMEOUT);
    let _ = join_with_timeout(reader, Duration::from_millis(500));
    let stderr = join_with_timeout(stderr_reader, Duration::from_millis(500)).unwrap_or_default();
    if let Err(error) = result {
        return Err(with_stderr(error, &stderr));
    }
    let status = exit.map_err(|error| with_stderr(error, &stderr))?;
    if !status.success() {
        return Err(with_stderr(
            format!("MCP server exited with {status}."),
            &stderr,
        ));
    }
    Ok(())
}

fn send_json(stdin: &mut impl Write, value: Value) -> Result<(), String> {
    writeln!(stdin, "{}", serde_json::to_string(&value).unwrap())
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("MCP handshake write failed: {error}"))
}

fn receive_response(
    receiver: &mpsc::Receiver<Result<String, String>>,
    expected_id: i64,
    stage: &str,
    budget: Duration,
) -> Result<Value, String> {
    let deadline = Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("MCP handshake timed out during {stage}."));
        }
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|error| format!("MCP handshake timed out during {stage}: {error}"))??;
        let value: Value = serde_json::from_str(&line).map_err(|error| {
            format!("MCP handshake returned invalid JSON during {stage}: {error}")
        })?;
        if value["id"].as_i64() == Some(expected_id) {
            return Ok(value);
        }
        if value.get("method").is_some() {
            continue;
        }
    }
}

fn run_output(command: &mut Command, timeout: Duration) -> Result<std::process::Output, String> {
    let mut stdout_capture = CommandCapture::create("stdout")?;
    let mut stderr_capture = CommandCapture::create("stderr")?;
    command
        .stdout(stdout_capture.stdio()?)
        .stderr(stderr_capture.stdio()?);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start command: {error}"))?;
    let status = wait_child(&mut child, timeout);
    let stdout = stdout_capture.read_all()?;
    let stderr = stderr_capture.read_all()?;
    let status = status.map_err(|error| with_stderr(error, &String::from_utf8_lossy(&stderr)))?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

struct CommandCapture {
    path: PathBuf,
    file: Option<File>,
}

impl CommandCapture {
    fn create(label: &str) -> Result<Self, String> {
        for _ in 0..64 {
            let sequence = CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fastctx-doctor-{}-{sequence}-{label}.tmp",
                std::process::id()
            ));
            match OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)
            {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "cannot create command capture file {}: {error}",
                        crate::paths::display_path(&path)
                    ));
                }
            }
        }
        Err("cannot allocate a unique command capture file".to_string())
    }

    fn stdio(&self) -> Result<Stdio, String> {
        self.file
            .as_ref()
            .ok_or_else(|| "command capture file is closed".to_string())?
            .try_clone()
            .map(Stdio::from)
            .map_err(|error| format!("cannot clone command capture file: {error}"))
    }

    fn read_all(&mut self) -> Result<Vec<u8>, String> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "command capture file is closed".to_string())?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("cannot rewind command capture: {error}"))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read command capture: {error}"))?;
        Ok(bytes)
    }
}

impl Drop for CommandCapture {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}

fn wait_child(child: &mut Child, timeout: Duration) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "command timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                terminate(child);
                return Err(format!("cannot wait for command: {error}"));
            }
        }
    }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn join_with_timeout<T: Send + 'static>(
    handle: thread::JoinHandle<T>,
    timeout: Duration,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !handle.is_finished() {
        return Err("reader thread did not finish before the cleanup deadline".to_string());
    }
    handle
        .join()
        .map_err(|_| "reader thread panicked".to_string())
}

fn output_detail(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    } else {
        stdout
    }
}

fn with_stderr(message: String, stderr: &str) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        message
    } else {
        format!("{message} Server stderr: {stderr}")
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
