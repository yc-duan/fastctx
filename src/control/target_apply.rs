//! Target-neutral Apply and Disconnect planning over immutable file transactions.

use crate::control::agents;
use crate::control::apply::{
    self, ApplyPlan, OperationReceipt, PreviewAction, PreviewDetail, PreviewItem, PreviewTarget,
};
use crate::control::codex_config::{self, ExpectedConfig};
use crate::control::paths::ControlPaths;
use crate::control::settings::{
    self, InstallationRecord, ManagedFileRecord, TargetReceipt, Tier, ToolBudgetPreferences,
};
use crate::control::targets::{
    AgentTarget, ConfigApplyRequest, apply_config, apply_guidance, disconnect_config,
    disconnect_guidance,
};
use crate::control::transaction::{self, FileAction, FileChange};
use crate::server_manifest::EnabledTools;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::format_description;

#[derive(Clone, Debug)]
pub struct TargetApplyOptions {
    pub target: AgentTarget,
    pub enabled_tools: EnabledTools,
    pub tier: Tier,
    pub tool_budgets: ToolBudgetPreferences,
    pub output_guard_enabled: bool,
    pub current_executable: PathBuf,
}

#[derive(Clone, Debug)]
pub enum TargetApplyPlan {
    Codex(ApplyPlan),
    Generic(GenericApplyPlan),
}

impl TargetApplyPlan {
    pub fn preview(&self) -> &[PreviewItem] {
        match self {
            Self::Codex(plan) => plan.preview(),
            Self::Generic(plan) => &plan.preview,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Codex(plan) => plan.is_empty(),
            Self::Generic(plan) => plan.changes.iter().all(|change| !change.is_changed()),
        }
    }

    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Self::Codex(plan) if plan.token_limit_conflict().is_some())
    }
}

#[derive(Clone, Debug)]
pub struct GenericApplyPlan {
    target: AgentTarget,
    changes: Vec<FileChange>,
    preview: Vec<PreviewItem>,
}

#[derive(Clone, Debug)]
pub struct TargetDisconnectPlan {
    target: AgentTarget,
    changes: Vec<FileChange>,
    preview: Vec<PreviewItem>,
    created_directories: Vec<PathBuf>,
}

impl TargetDisconnectPlan {
    pub fn preview(&self) -> &[PreviewItem] {
        &self.preview
    }

    pub fn is_empty(&self) -> bool {
        self.changes.iter().all(|change| !change.is_changed())
    }

    pub(crate) fn into_host_changes(self, settings_path: &Path) -> (Vec<FileChange>, Vec<PathBuf>) {
        (
            self.changes
                .into_iter()
                .filter(|change| change.target != settings_path)
                .collect(),
            self.created_directories,
        )
    }
}

pub fn plan_target_apply(
    control: &ControlPaths,
    options: TargetApplyOptions,
) -> Result<TargetApplyPlan, String> {
    if options.target == AgentTarget::Codex {
        return apply::plan_apply(
            control,
            apply::ApplyOptions {
                tier: options.tier,
                tool_budgets: options.tool_budgets,
                output_guard_enabled: options.output_guard_enabled,
                enabled_tools: options.enabled_tools,
                current_executable: options.current_executable,
            },
        )
        .map(TargetApplyPlan::Codex);
    }
    if options.enabled_tools.shell_enabled() {
        crate::shell::bash::probe_bash().map_err(|error| {
            format!(
                "The shell suite is enabled, but Apply cannot continue: {error} Disable the shell suite or fix bash first."
            )
        })?;
    }
    let target_paths = options.target.paths(control)?;
    let source_binary = fs::read(&options.current_executable).map_err(|error| {
        format!(
            "Cannot read the running FastCtx binary {}: {error}",
            crate::paths::display_path(&options.current_executable)
        )
    })?;
    let binary_hash = sha256(&source_binary);
    let installed_original = if same_path(&options.current_executable, &control.installed_binary) {
        Some(source_binary.clone())
    } else {
        transaction::read_snapshot(&control.installed_binary)?
    };
    let config_original = transaction::read_snapshot(&target_paths.config)?;
    let guidance_original = transaction::read_snapshot(&target_paths.guidance)?;
    let settings_original = transaction::read_snapshot(&control.fastctx_config)?;
    let mut current = settings::load(control)?;
    if settings_original.is_none() {
        current.last_seen_version = Some(env!("CARGO_PKG_VERSION").to_string());
        current.tool_budget_epoch = Some(settings::TOOL_BUDGET_EPOCH);
    }
    let previous = current.target_receipt(options.target).cloned();
    validate_receipt_paths(
        previous.as_ref(),
        &target_paths.config,
        &target_paths.guidance,
    )?;
    let selected_budget = options.tier.fastctx_budget();
    let global_budget = options.target.budget_policy().clamp(selected_budget);
    let resolved_budgets = options.tool_budgets.resolve(options.tier);
    let config_edit = apply_config(ConfigApplyRequest {
        target: options.target,
        original: config_original.as_deref().unwrap_or_default(),
        executable: &control.installed_binary,
        tools: options.enabled_tools,
        global_budget,
        tool_budgets: resolved_budgets,
        owned_hash: previous
            .as_ref()
            .map(|receipt| receipt.config_entry_sha256.as_str()),
        previous_jsonc: previous
            .as_ref()
            .and_then(|receipt| receipt.jsonc_config.as_ref()),
    })?;
    let guidance_edit = apply_guidance(
        options.target,
        guidance_original.as_deref(),
        options.enabled_tools,
        previous.is_some(),
    )?;
    let mut created_directories = previous
        .as_ref()
        .map(|receipt| receipt.created_directories.clone())
        .unwrap_or_default();
    for path in [&target_paths.config, &target_paths.guidance] {
        created_directories.extend(missing_parent_directories(path, &control.home)?);
    }
    created_directories.sort();
    created_directories.dedup();
    let applied_at_utc = timestamp()?;
    current.installation = Some(InstallationRecord {
        version: env!("CARGO_PKG_VERSION").to_string(),
        command: crate::paths::display_path(&control.installed_binary),
        binary_sha256: binary_hash,
    });
    let receipt = TargetReceipt {
        applied_at_utc,
        version: env!("CARGO_PKG_VERSION").to_string(),
        enabled_tools: options.enabled_tools,
        fastctx_token_budget: global_budget,
        config: managed_record(
            &target_paths.config,
            &config_original,
            &config_edit.bytes,
            previous.as_ref().map(|receipt| &receipt.config),
        ),
        guidance: managed_record(
            &target_paths.guidance,
            &guidance_original,
            &guidance_edit.bytes,
            previous.as_ref().map(|receipt| &receipt.guidance),
        ),
        config_entry_sha256: config_edit.managed_hash,
        guidance_managed_sha256: guidance_edit.managed_hash,
        guidance_inserted_separator: previous
            .as_ref()
            .and_then(|receipt| receipt.guidance_inserted_separator)
            .or(guidance_edit.inserted_separator),
        created_directories,
        jsonc_config: config_edit.jsonc_receipt,
        codex: None,
    };
    current.set_target_receipt(options.target, receipt);
    let settings_bytes = settings::encode(&current)?;
    let changes = vec![
        file_write(
            control.installed_binary.clone(),
            installed_original,
            source_binary,
            Some(0o755),
            true,
        ),
        file_write(
            target_paths.config.clone(),
            config_original,
            config_edit.bytes,
            transaction::existing_unix_mode(&target_paths.config).or(Some(0o600)),
            false,
        ),
        file_write(
            target_paths.guidance.clone(),
            guidance_original,
            guidance_edit.bytes,
            transaction::existing_unix_mode(&target_paths.guidance).or(Some(0o600)),
            false,
        ),
        file_write(
            control.fastctx_config.clone(),
            settings_original,
            settings_bytes,
            transaction::existing_unix_mode(&control.fastctx_config).or(Some(0o600)),
            false,
        ),
    ];
    let preview = generic_apply_preview(GenericApplyPreview {
        target: options.target,
        tools: options.enabled_tools,
        budget: global_budget,
        changes: &changes,
        config: &target_paths.config,
        guidance: &target_paths.guidance,
        binary: &control.installed_binary,
        settings: &control.fastctx_config,
    });
    Ok(TargetApplyPlan::Generic(GenericApplyPlan {
        target: options.target,
        changes,
        preview,
    }))
}

pub fn commit_target_apply(
    plan: TargetApplyPlan,
    token_limit_confirmed: bool,
) -> Result<OperationReceipt, String> {
    match plan {
        TargetApplyPlan::Codex(plan) => apply::commit_apply(plan, token_limit_confirmed),
        TargetApplyPlan::Generic(plan) => {
            let changed_targets = plan
                .changes
                .iter()
                .filter(|change| change.is_changed())
                .count();
            transaction::commit(&plan.changes)?;
            Ok(OperationReceipt {
                changed_targets,
                notes: vec![if changed_targets == 0 {
                    "No changes were needed.".to_string()
                } else {
                    format!(
                        "{} will use the connection in newly started sessions.",
                        plan.target.display_name()
                    )
                }],
            })
        }
    }
}

pub fn plan_target_disconnect(
    control: &ControlPaths,
    target: AgentTarget,
) -> Result<TargetDisconnectPlan, String> {
    if target == AgentTarget::Codex {
        return plan_codex_disconnect(control);
    }
    let target_paths = target.paths(control)?;
    let settings_original = transaction::read_snapshot(&control.fastctx_config)?;
    let mut current = settings::load(control)?;
    let receipt = current.target_receipt(target).cloned().ok_or_else(|| {
        format!(
            "{} is not connected; there is no receipt to Disconnect.",
            target.display_name()
        )
    })?;
    validate_receipt_paths(Some(&receipt), &target_paths.config, &target_paths.guidance)?;
    let config_original = transaction::read_snapshot(&target_paths.config)?.ok_or_else(|| {
        format!(
            "Managed config {} is missing; Disconnect stopped before changing anything.",
            crate::paths::display_path(&target_paths.config)
        )
    })?;
    let guidance_original =
        transaction::read_snapshot(&target_paths.guidance)?.ok_or_else(|| {
            format!(
                "Managed guidance {} is missing; Disconnect stopped before changing anything.",
                crate::paths::display_path(&target_paths.guidance)
            )
        })?;
    let config_bytes = disconnect_config(
        target,
        &config_original,
        &receipt.config_entry_sha256,
        receipt.jsonc_config.as_ref(),
    )?;
    let guidance_bytes = disconnect_guidance(
        target,
        &guidance_original,
        &receipt.guidance_managed_sha256,
        receipt.guidance_inserted_separator,
        receipt.guidance.original_existed,
    )?;
    current.remove_target_receipt(target);
    let settings_bytes = settings::encode(&current)?;
    let config_action =
        if !receipt.config.original_existed && config_is_empty(target, &config_bytes) {
            FileAction::Delete
        } else {
            FileAction::Write(config_bytes)
        };
    let guidance_action = match guidance_bytes {
        None => FileAction::Delete,
        Some(bytes) if !receipt.guidance.original_existed && bytes.is_empty() => FileAction::Delete,
        Some(bytes) => FileAction::Write(bytes),
    };
    let changes = vec![
        FileChange {
            target: target_paths.config.clone(),
            original: Some(config_original),
            action: config_action,
            unix_mode: transaction::existing_unix_mode(&target_paths.config).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: target_paths.guidance.clone(),
            original: Some(guidance_original),
            action: guidance_action,
            unix_mode: transaction::existing_unix_mode(&target_paths.guidance).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        file_write(
            control.fastctx_config.clone(),
            settings_original,
            settings_bytes,
            transaction::existing_unix_mode(&control.fastctx_config).or(Some(0o600)),
            false,
        ),
    ];
    let preview = disconnect_preview(
        target,
        &changes,
        &target_paths.config,
        &target_paths.guidance,
    );
    Ok(TargetDisconnectPlan {
        target,
        changes,
        preview,
        created_directories: receipt
            .created_directories
            .into_iter()
            .map(PathBuf::from)
            .collect(),
    })
}

pub fn commit_target_disconnect(plan: TargetDisconnectPlan) -> Result<OperationReceipt, String> {
    let changed_targets = plan
        .changes
        .iter()
        .filter(|change| change.is_changed())
        .count();
    transaction::commit(&plan.changes)?;
    let mut directories = plan.created_directories;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    let mut notes = Vec::new();
    for directory in directories {
        match fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => notes.push(format!(
                "Could not remove the empty FastCtx-created directory {}: {error}.",
                crate::paths::display_path(&directory)
            )),
        }
    }
    notes.push(format!("Disconnected {}.", plan.target.display_name()));
    Ok(OperationReceipt {
        changed_targets,
        notes,
    })
}

fn plan_codex_disconnect(control: &ControlPaths) -> Result<TargetDisconnectPlan, String> {
    let settings_original = transaction::read_snapshot(&control.fastctx_config)?;
    let mut current = settings::load(control)?;
    let receipt = current
        .target_receipt(AgentTarget::Codex)
        .cloned()
        .ok_or_else(|| "Codex is not connected; there is no receipt to Disconnect.".to_string())?;
    let record = current
        .applied
        .clone()
        .ok_or_else(|| "The Codex receipt is incomplete; run Apply to repair it.".to_string())?;
    if !record.targets_codex_profile(control) {
        return Err(
            "The Codex receipt targets a different profile; select that profile before Disconnect."
                .to_string(),
        );
    }
    let config_original = transaction::read_snapshot(&control.codex_config)?.ok_or_else(|| {
        "The managed Codex config is missing; Disconnect stopped before changing anything."
            .to_string()
    })?;
    let expected = ExpectedConfig {
        command: record.command.clone(),
        tier: record.tier,
        host_limit: record.tool_output_token_limit,
        fastctx_budget: record.fastctx_token_budget,
        tool_budgets: record.tool_budgets,
        enabled_tools: receipt.enabled_tools,
    };
    let drift = codex_config::drift_applied(
        &config_original,
        &expected,
        record.tool_output_token_limit,
        record.fastctx_token_budget,
        record.tool_timeout_sec,
    )?;
    if !drift.is_empty() {
        return Err(format!(
            "Codex managed configuration drifted after Apply ({}); Disconnect will not remove user-changed values.",
            drift.join(", ")
        ));
    }
    let restore_token_limit =
        codex_config::current_token_limit(&config_original) == Some(record.tool_output_token_limit);
    let config_bytes = codex_config::unapply(
        &config_original,
        codex_config::CodexConfigOwnership {
            server_entry_owned: record.codex_server_entry_owned,
            direct_namespace_inserted: record.codex_direct_namespace_inserted,
        },
        restore_token_limit,
        record.previous_token_limit_present,
        record.previous_token_limit,
    )?;
    let guidance_original =
        transaction::read_snapshot(&control.codex_agents)?.ok_or_else(|| {
            "The managed Codex AGENTS.md is missing; Disconnect stopped before changing anything."
                .to_string()
        })?;
    if agents::classify_managed_section_for_tools(&guidance_original, receipt.enabled_tools)
        != agents::ManagedSectionState::Current
    {
        return Err(
            "Codex managed guidance drifted after Apply; Disconnect will not remove user-changed bytes."
                .to_string(),
        );
    }
    let guidance_bytes =
        agents::remove_applied_section(&guidance_original, receipt.guidance_inserted_separator)?;
    current.remove_target_receipt(AgentTarget::Codex);
    let settings_bytes = settings::encode(&current)?;
    let config_action = if config_bytes.is_empty() && !receipt.config.original_existed {
        FileAction::Delete
    } else {
        FileAction::Write(config_bytes)
    };
    let guidance_action = if guidance_bytes.is_empty() && !receipt.guidance.original_existed {
        FileAction::Delete
    } else {
        FileAction::Write(guidance_bytes)
    };
    let changes = vec![
        FileChange {
            target: control.codex_config.clone(),
            original: Some(config_original),
            action: config_action,
            unix_mode: transaction::existing_unix_mode(&control.codex_config).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: control.codex_agents.clone(),
            original: Some(guidance_original),
            action: guidance_action,
            unix_mode: transaction::existing_unix_mode(&control.codex_agents).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        file_write(
            control.fastctx_config.clone(),
            settings_original,
            settings_bytes,
            transaction::existing_unix_mode(&control.fastctx_config).or(Some(0o600)),
            false,
        ),
    ];
    let preview = disconnect_preview(
        AgentTarget::Codex,
        &changes,
        &control.codex_config,
        &control.codex_agents,
    );
    Ok(TargetDisconnectPlan {
        target: AgentTarget::Codex,
        changes,
        preview,
        created_directories: receipt
            .created_directories
            .into_iter()
            .map(PathBuf::from)
            .collect(),
    })
}

struct GenericApplyPreview<'a> {
    target: AgentTarget,
    tools: EnabledTools,
    budget: usize,
    changes: &'a [FileChange],
    config: &'a Path,
    guidance: &'a Path,
    binary: &'a Path,
    settings: &'a Path,
}

fn generic_apply_preview(input: GenericApplyPreview<'_>) -> Vec<PreviewItem> {
    let GenericApplyPreview {
        target,
        tools,
        budget,
        changes,
        config,
        guidance,
        binary,
        settings,
    } = input;
    changes
        .iter()
        .map(|change| {
            let (target_kind, action, details) = if change.target == binary {
                (
                    PreviewTarget::Binary,
                    PreviewAction::Install,
                    vec![PreviewDetail::kept(format!(
                        "fastctx v{}",
                        env!("CARGO_PKG_VERSION")
                    ))],
                )
            } else if change.target == config {
                (
                    PreviewTarget::AgentConfig,
                    PreviewAction::Modify,
                    vec![
                        PreviewDetail::kept(format!("target = {}", target.id())),
                        PreviewDetail::kept(format!("tools = {}", tools.names().join(","))),
                        PreviewDetail::kept(format!("FASTCTX_TOKEN_BUDGET = {budget}")),
                    ],
                )
            } else if change.target == guidance {
                (
                    PreviewTarget::AgentGuidance,
                    PreviewAction::Modify,
                    vec![PreviewDetail::kept("generated enabled-tool guidance")],
                )
            } else if change.target == settings {
                (
                    PreviewTarget::Receipt,
                    PreviewAction::Record,
                    vec![PreviewDetail::kept(format!(
                        "applied_targets.{}",
                        target.id()
                    ))],
                )
            } else {
                (PreviewTarget::Receipt, PreviewAction::Modify, Vec::new())
            };
            PreviewItem {
                path: change.target.clone(),
                action: if change.is_changed() {
                    action
                } else {
                    PreviewAction::Unchanged
                },
                target: target_kind,
                details,
            }
        })
        .collect()
}

fn disconnect_preview(
    target: AgentTarget,
    changes: &[FileChange],
    config: &Path,
    guidance: &Path,
) -> Vec<PreviewItem> {
    changes
        .iter()
        .map(|change| PreviewItem {
            path: change.target.clone(),
            action: if !change.is_changed() {
                PreviewAction::Unchanged
            } else if matches!(change.action, FileAction::Delete) {
                PreviewAction::Delete
            } else {
                PreviewAction::Modify
            },
            target: if change.target == config {
                PreviewTarget::AgentConfig
            } else if change.target == guidance {
                PreviewTarget::AgentGuidance
            } else {
                PreviewTarget::Receipt
            },
            details: vec![PreviewDetail::removed(format!(
                "{} connection ownership",
                target.id()
            ))],
        })
        .collect()
}

fn validate_receipt_paths(
    receipt: Option<&TargetReceipt>,
    config: &Path,
    guidance: &Path,
) -> Result<(), String> {
    let Some(receipt) = receipt else {
        return Ok(());
    };
    if !same_path(Path::new(&receipt.config.path), config)
        || !same_path(Path::new(&receipt.guidance.path), guidance)
    {
        return Err(
            "The target receipt points at different config or guidance paths; FastCtx will not reuse ownership evidence across profiles."
                .to_string(),
        );
    }
    Ok(())
}

fn managed_record(
    path: &Path,
    original: &Option<Vec<u8>>,
    applied: &[u8],
    previous: Option<&ManagedFileRecord>,
) -> ManagedFileRecord {
    ManagedFileRecord {
        path: crate::paths::display_path(path),
        original_existed: previous
            .map(|record| record.original_existed)
            .unwrap_or_else(|| original.is_some()),
        applied_sha256: sha256(applied),
    }
}

fn missing_parent_directories(path: &Path, home: &Path) -> Result<Vec<String>, String> {
    let mut missing = Vec::new();
    let mut current = path.parent();
    while let Some(directory) = current {
        if !directory.starts_with(home) {
            break;
        }
        match fs::metadata(directory) {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(format!(
                    "Cannot create target files because {} is not a directory.",
                    crate::paths::display_path(directory)
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(crate::paths::display_path(directory));
            }
            Err(error) => {
                return Err(format!(
                    "Cannot inspect target directory {}: {error}",
                    crate::paths::display_path(directory)
                ));
            }
        }
        current = directory.parent();
    }
    Ok(missing)
}

fn config_is_empty(target: AgentTarget, bytes: &[u8]) -> bool {
    let trimmed = String::from_utf8_lossy(bytes);
    let trimmed = trimmed.trim();
    match target {
        AgentTarget::Trae => trimmed.is_empty() || trimmed == "mcp_servers:",
        _ => trimmed.is_empty() || trimmed == "{}",
    }
}

fn file_write(
    target: PathBuf,
    original: Option<Vec<u8>>,
    bytes: Vec<u8>,
    unix_mode: Option<u32>,
    locked_binary_fallback: bool,
) -> FileChange {
    FileChange {
        target,
        original,
        action: FileAction::Write(bytes),
        unix_mode,
        locked_binary_fallback,
    }
}

fn timestamp() -> Result<String, String> {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second].[subsecond digits:9]Z"
        ))
        .map_err(|error| format!("Cannot format the Apply timestamp: {error}"))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
