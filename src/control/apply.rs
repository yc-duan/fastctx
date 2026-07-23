//! Immutable Apply/Unapply plans, previews, and transaction commits.

use crate::control::agents;
use crate::control::codex_config::{self, ExpectedConfig, TokenLimitConflict};
use crate::control::paths::ControlPaths;
use crate::control::processes::{self, InstalledProcess, TerminationOutcome};
use crate::control::settings::{self, AppliedRecord, ManagedFileRecord, Tier, ToolBudgets};
use crate::control::transaction::{self, FileAction, FileChange};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::format_description;

/// User choices for Apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyOptions {
    /// Host output tier.
    pub tier: Tier,
    /// Five long-output tools' advanced budgets.
    pub tool_budgets: ToolBudgets,
    /// Whether the optional shell tool group should be published.
    pub fastshell_enabled: bool,
    /// Currently running binary to self-install.
    pub current_executable: PathBuf,
}

/// User choices for Unapply. Unapply is a complete removal with no options: the token key
/// is restored by ownership and `~/.fastctx/` is always deleted (2026-07-12).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnapplyOptions {
    /// Current process path, used to avoid Windows self-deletion failures.
    pub current_executable: PathBuf,
}

/// Action semantics for a preview item; the TUI localizes verbs while CLI output remains stable English.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewAction {
    /// Install or update the binary at its stable path.
    Install,
    /// Minimally edit an existing user file.
    Modify,
    /// Write FastCtx's own receipt.
    Record,
    /// Delete a file or data.
    Delete,
    /// Keep a running binary that cannot delete itself and request manual cleanup.
    Keep,
    /// Target already has the desired shape, so this operation writes nothing.
    Unchanged,
}

impl PreviewAction {
    /// Stable English verb used by CLI output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Modify => "Modify",
            Self::Record => "Record",
            Self::Delete => "Delete",
            Self::Keep => "Keep",
            Self::Unchanged => "Unchanged",
        }
    }
}

/// Semantic target category used by the TUI for purpose text; CLI output is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewTarget {
    /// FastCtx executable under `~/.fastctx/bin`.
    Binary,
    /// `~/.codex/config.toml`: register the MCP server and keep it directly visible to the model.
    CodexConfig,
    /// `~/.codex/AGENTS.md`: guide the model to prefer FastCtx.
    Agents,
    /// `~/.fastctx/config.toml`: Apply receipt used for removal.
    Receipt,
}

/// One target in a preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewItem {
    /// Absolute target path.
    pub path: PathBuf,
    /// Action semantics.
    pub action: PreviewAction,
    /// Target category selecting the purpose text shown by the TUI.
    pub target: PreviewTarget,
    /// Technical detail lines: config keys and values or scalar old-to-new changes; literals remain English.
    pub details: Vec<PreviewDetail>,
}

/// One preview detail line. Removed lines use TUI strikethrough and a CLI `-` prefix to show
/// that the item will disappear; keys and values remain literal English.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewDetail {
    /// Detail text.
    pub text: String,
    /// Whether this item will be removed.
    pub removed: bool,
}

impl PreviewDetail {
    /// Detail for writing, modifying, or retaining an item.
    fn kept(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            removed: false,
        }
    }

    /// Removal detail for tables, sections, receipts, or keys deleted by Unapply.
    fn removed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            removed: true,
        }
    }
}

/// Immutable Apply ChangeSet shared by preview and commit.
#[derive(Clone, Debug)]
pub struct ApplyPlan {
    changes: Vec<FileChange>,
    preview: Vec<PreviewItem>,
    conflict: Option<TokenLimitConflict>,
    stale_binaries: Vec<PathBuf>,
}

impl ApplyPlan {
    /// Returns every effective change; an empty set means an idempotent no-op.
    pub fn preview(&self) -> &[PreviewItem] {
        &self.preview
    }

    /// Returns a shared token-key conflict that requires explicit confirmation before commit.
    pub fn token_limit_conflict(&self) -> Option<&TokenLimitConflict> {
        self.conflict.as_ref()
    }

    /// Whether no file or binary would change.
    pub fn is_empty(&self) -> bool {
        self.stale_binaries.is_empty() && self.changes.iter().all(|change| !change.is_changed())
    }
}

/// Immutable Unapply ChangeSet.
#[derive(Clone, Debug)]
pub struct UnapplyPlan {
    changes: Vec<FileChange>,
    preview: Vec<PreviewItem>,
    fastctx_dir: PathBuf,
    codex_dir_cleanup: Option<PathBuf>,
    manual_binary_cleanup: Option<PathBuf>,
    paths: ControlPaths,
    running_jobs: usize,
    running_processes: Vec<InstalledProcess>,
}

impl UnapplyPlan {
    /// Returns every effective change.
    pub fn preview(&self) -> &[PreviewItem] {
        &self.preview
    }

    /// Whether no file would change.
    pub fn is_empty(&self) -> bool {
        self.running_jobs == 0
            && self.running_processes.is_empty()
            && self.changes.iter().all(|change| !change.is_changed())
    }

    /// Number of running jobs observed by the immutable preview.
    pub const fn running_jobs(&self) -> usize {
        self.running_jobs
    }

    /// Number of installed FastCtx process images that confirmation will terminate.
    pub fn running_processes(&self) -> usize {
        self.running_processes.len()
    }
}

/// Receipt for a completed operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationReceipt {
    /// Number of targets actually changed.
    pub changed_targets: usize,
    /// Final user-facing notice.
    pub notes: Vec<String>,
}

/// Outcome of synchronizing the stable Codex binary after an explicit product update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AppliedBinarySync {
    /// Apply has not installed a stable binary.
    NotApplied,
    /// The stable binary and receipt already match the running version.
    Unchanged,
    /// The stable binary and receipt were advanced atomically.
    Updated,
}

/// Advances an owned stable binary and its receipt without changing shared Codex files.
pub(crate) fn synchronize_applied_binary(
    paths: &ControlPaths,
    current_executable: &Path,
) -> Result<AppliedBinarySync, String> {
    let settings_original = transaction::read_snapshot(&paths.fastctx_config)?;
    let mut current_settings = settings::load(paths)?;
    let Some(record) = current_settings.applied.as_mut() else {
        return Ok(AppliedBinarySync::NotApplied);
    };
    if !same_path(Path::new(&record.command), &paths.installed_binary) {
        return Err(
            "the Apply receipt points at a different stable binary; run Apply manually".to_string(),
        );
    }
    let installed_original = transaction::read_snapshot(&paths.installed_binary)?
        .ok_or_else(|| "the applied stable binary is missing; run Apply manually".to_string())?;
    if sha256(&installed_original) != record.binary_sha256 {
        return Err(
            "the applied stable binary changed outside FastCtx; run Apply manually".to_string(),
        );
    }
    let source_binary = fs::read(current_executable).map_err(|error| {
        format!(
            "Cannot read the updated FastCtx binary {}: {error}",
            crate::paths::display_path(current_executable)
        )
    })?;
    let source_hash = sha256(&source_binary);
    if installed_original == source_binary
        && record.binary_sha256 == source_hash
        && record.version == env!("CARGO_PKG_VERSION")
    {
        return Ok(AppliedBinarySync::Unchanged);
    }
    record.version = env!("CARGO_PKG_VERSION").to_string();
    record.binary_sha256 = source_hash;
    let settings_bytes = settings::encode(&current_settings)?;
    let changes = [
        file_write(
            paths.installed_binary.clone(),
            Some(installed_original),
            source_binary,
            transaction::existing_unix_mode(&paths.installed_binary).or(Some(0o755)),
            true,
        ),
        file_write(
            paths.fastctx_config.clone(),
            settings_original,
            settings_bytes,
            transaction::existing_unix_mode(&paths.fastctx_config).or(Some(0o600)),
            false,
        ),
    ];
    transaction::commit(&changes)?;
    Ok(AppliedBinarySync::Updated)
}

/// Computes the complete immutable Apply plan without writing to disk.
pub fn plan_apply(paths: &ControlPaths, options: ApplyOptions) -> Result<ApplyPlan, String> {
    if options.fastshell_enabled {
        crate::shell::bash::probe_bash().map_err(|error| {
            format!(
                "fastshell is enabled, but Apply cannot continue: {error} Disable fastshell in Config and retry, or fix bash first."
            )
        })?;
    }
    let codex_dir_missing = codex_directory_will_be_created(paths)?;
    let source_binary = fs::read(&options.current_executable).map_err(|error| {
        format!(
            "Cannot read the running fastctx binary {}: {error}",
            crate::paths::display_path(&options.current_executable)
        )
    })?;
    let binary_hash = sha256(&source_binary);
    let timestamp = timestamp()?;

    let codex_original = transaction::read_snapshot(&paths.codex_config)?;
    let codex_source = codex_original.as_deref().unwrap_or_default();
    let expected = ExpectedConfig {
        command: crate::paths::display_path(&paths.installed_binary),
        tier: options.tier,
        tool_budgets: options.tool_budgets,
        fastshell_enabled: options.fastshell_enabled,
    };
    let codex_edit = codex_config::apply(codex_source, &expected)?;

    let agents_original = transaction::read_snapshot(&paths.codex_agents)?;
    let agents_edit = agents::apply_section_with_ownership_for(
        agents_original.as_deref().unwrap_or_default(),
        options.fastshell_enabled,
    )?;
    let agents_bytes = agents_edit.bytes;

    let installed_original = if same_path(&options.current_executable, &paths.installed_binary) {
        Some(source_binary.clone())
    } else {
        transaction::read_snapshot(&paths.installed_binary)?
    };

    let settings_original = transaction::read_snapshot(&paths.fastctx_config)?;
    let mut current_settings = settings::load(paths)?;
    if let Some(record) = current_settings
        .applied
        .as_ref()
        .filter(|record| !record.targets_codex_profile(paths))
    {
        return Err(receipt_profile_mismatch(paths, record));
    }
    let previous_applied = current_settings
        .applied
        .clone()
        .filter(|record| record.targets_codex_profile(paths));
    let codex_dir_created = previous_applied
        .as_ref()
        .map(|record| record.codex_dir_created || codex_dir_missing)
        .unwrap_or(codex_dir_missing);
    let agents_inserted_separator = previous_applied
        .as_ref()
        .filter(|record| {
            agents_original
                .as_deref()
                .is_some_and(|bytes| record.codex_agents.applied_sha256 == sha256(bytes))
        })
        .and_then(|record| record.codex_agents_inserted_separator)
        .or(agents_edit.inserted_separator);
    let managed_unchanged = codex_original.as_deref() == Some(codex_edit.bytes.as_slice())
        && agents_original.as_deref() == Some(agents_bytes.as_slice())
        && installed_original.as_deref() == Some(source_binary.as_slice());
    let record_current = previous_applied.as_ref().is_some_and(|record| {
        record_matches(
            record,
            &RecordMatchContext {
                expected: &expected,
                paths,
                binary_hash: &binary_hash,
                agents_bytes: &agents_bytes,
                agents_inserted_separator,
                codex_dir_created,
            },
        )
    });
    let keep_settings_bytes = managed_unchanged
        && record_current
        && current_settings.tier == options.tier
        && current_settings.tool_budgets == options.tool_budgets
        && current_settings.fastshell.enabled == options.fastshell_enabled;

    let settings_bytes = if keep_settings_bytes {
        settings_original.clone().unwrap_or_else(|| {
            settings::encode(&current_settings)
                .expect("a previously parsed fastctx settings file must serialize")
        })
    } else {
        current_settings.tier = options.tier;
        current_settings.tool_budgets = options.tool_budgets;
        current_settings.fastshell.enabled = options.fastshell_enabled;
        current_settings.fastedit.enabled = false;
        let (previous_token_limit_present, previous_token_limit) = previous_applied
            .as_ref()
            .map(|record| {
                (
                    record.previous_token_limit_present,
                    record.previous_token_limit,
                )
            })
            .unwrap_or((
                codex_edit.previous_token_limit_present,
                codex_edit.previous_token_limit,
            ));
        current_settings.applied = Some(AppliedRecord {
            applied_at_utc: timestamp.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: expected.command.clone(),
            tier: options.tier,
            tool_output_token_limit: options.tier.host_limit(),
            tool_timeout_sec: Some(codex_config::TOOL_TIMEOUT_SECONDS),
            previous_token_limit_present,
            previous_token_limit,
            fastctx_token_budget: options.tier.fastctx_budget(),
            tool_budgets: options.tool_budgets,
            fastshell_enabled: options.fastshell_enabled,
            fastedit_enabled: false,
            codex_dir_created,
            codex_config: managed_record(
                &paths.codex_config,
                &codex_original,
                &codex_edit.bytes,
                previous_applied.as_ref().map(|record| &record.codex_config),
            ),
            codex_agents: managed_record(
                &paths.codex_agents,
                &agents_original,
                &agents_bytes,
                previous_applied.as_ref().map(|record| &record.codex_agents),
            ),
            codex_agents_inserted_separator: agents_inserted_separator,
            binary_sha256: binary_hash,
        });
        settings::encode(&current_settings)?
    };

    let changes = vec![
        file_write(
            paths.installed_binary.clone(),
            installed_original,
            source_binary,
            Some(0o755),
            true,
        ),
        file_write(
            paths.codex_config.clone(),
            codex_original,
            codex_edit.bytes,
            transaction::existing_unix_mode(&paths.codex_config).or(Some(0o600)),
            false,
        ),
        file_write(
            paths.codex_agents.clone(),
            agents_original,
            agents_bytes,
            transaction::existing_unix_mode(&paths.codex_agents).or(Some(0o600)),
            false,
        ),
        file_write(
            paths.fastctx_config.clone(),
            settings_original,
            settings_bytes,
            transaction::existing_unix_mode(&paths.fastctx_config).or(Some(0o600)),
            false,
        ),
    ];
    let stale_binaries = find_stale_binaries(paths, &options.current_executable)?;
    let mut preview = preview_apply(
        &changes,
        &expected,
        codex_edit.previous_token_limit_present,
        codex_edit.previous_token_limit,
    );
    preview.extend(stale_binaries.iter().cloned().map(|path| PreviewItem {
        path,
        action: PreviewAction::Delete,
        target: PreviewTarget::Binary,
        details: vec![PreviewDetail::removed("stale self-update leftover")],
    }));
    Ok(ApplyPlan {
        changes,
        preview,
        conflict: codex_edit.conflict,
        stale_binaries,
    })
}

/// Commits an existing Apply plan without recomputing file contents after preview.
pub fn commit_apply(
    plan: ApplyPlan,
    token_limit_confirmed: bool,
) -> Result<OperationReceipt, String> {
    if let Some(conflict) = &plan.conflict
        && !token_limit_confirmed
    {
        return Err(format!(
            "tool_output_token_limit is currently {} but the selected tier requires {}. Re-run with --yes or confirm this shared setting in the TUI.",
            conflict.current, conflict.requested
        ));
    }
    let mut changed_targets = plan
        .changes
        .iter()
        .filter(|change| change.is_changed())
        .count();
    transaction::commit(&plan.changes)?;
    let mut notes = Vec::new();
    for stale in plan.stale_binaries {
        match fs::remove_file(&stale) {
            Ok(()) => changed_targets += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => notes.push(format!(
                "Could not remove stale binary {}: {error}. A later Apply will retry.",
                crate::paths::display_path(&stale)
            )),
        }
    }
    if changed_targets == 0 {
        notes.push("No changes were needed.".to_string());
    } else {
        notes.push("Changes apply to newly started ChatGPT/Codex sessions.".to_string());
    }
    Ok(OperationReceipt {
        changed_targets,
        notes,
    })
}

/// Computes the complete immutable Unapply plan.
pub fn plan_unapply(paths: &ControlPaths, options: UnapplyOptions) -> Result<UnapplyPlan, String> {
    let running_jobs = crate::shell::jobs::running_summaries(paths)?.len();
    let running_processes = processes::installed_processes(&paths.fastctx_bin_dir)?
        .into_iter()
        .filter(|process| process.identity.pid != std::process::id())
        .collect::<Vec<_>>();
    let settings_original = transaction::read_snapshot(&paths.fastctx_config)?;
    let loaded_settings = settings::load(paths)?;
    if let Some(record) = loaded_settings
        .applied
        .as_ref()
        .filter(|record| !record.targets_codex_profile(paths))
    {
        return Err(receipt_profile_mismatch(paths, record));
    }
    let applied = loaded_settings.applied;
    let codex_dir_cleanup = applied
        .as_ref()
        .is_some_and(|record| record.codex_dir_created)
        .then(|| paths.codex_dir.clone());

    let codex_original = transaction::read_snapshot(&paths.codex_config)?;
    let codex_source = codex_original.as_deref().unwrap_or_default();
    // token_limit is a shared global key: restore it only while it still equals the value written by Apply.
    // Otherwise leave it untouched under the same "undo only what we wrote and the user did not change" rule.
    let restore_token_limit = applied.as_ref().is_some_and(|record| {
        codex_config::current_token_limit(codex_source) == Some(record.tool_output_token_limit)
    });
    let codex_bytes = match applied.as_ref() {
        Some(record) => codex_config::unapply(
            codex_source,
            restore_token_limit,
            record.previous_token_limit_present,
            record.previous_token_limit,
        )?,
        None => codex_config::unapply(codex_source, false, false, None)?,
    };
    let codex_original_existed = applied
        .as_ref()
        .map(|record| record.codex_config.original_existed)
        .unwrap_or_else(|| codex_original.is_some());
    let codex_action = if codex_bytes.is_empty() && !codex_original_existed {
        FileAction::Delete
    } else {
        FileAction::Write(codex_bytes)
    };

    let agents_original = transaction::read_snapshot(&paths.codex_agents)?;
    let agents_inserted_separator = applied
        .as_ref()
        .filter(|record| {
            agents_original
                .as_deref()
                .is_some_and(|bytes| record.codex_agents.applied_sha256 == sha256(bytes))
        })
        .and_then(|record| record.codex_agents_inserted_separator);
    let agents_bytes = agents::remove_applied_section(
        agents_original.as_deref().unwrap_or_default(),
        agents_inserted_separator,
    )?;
    // Delete a now-empty file only when Apply originally created it; otherwise write back the remaining content.
    let agents_original_existed = applied
        .as_ref()
        .map(|record| record.codex_agents.original_existed)
        .unwrap_or_else(|| agents_original.is_some());
    let agents_action =
        if agents_original.is_none() || (agents_bytes.is_empty() && !agents_original_existed) {
            FileAction::Delete
        } else {
            FileAction::Write(agents_bytes)
        };

    let installed_original = transaction::read_snapshot(&paths.installed_binary)?;
    let running_installed =
        cfg!(windows) && same_path(&options.current_executable, &paths.installed_binary);
    let manual_binary_cleanup = (cfg!(windows)
        && processes::path_is_under(&options.current_executable, &paths.fastctx_bin_dir)
        && options.current_executable.exists())
    .then(|| options.current_executable.clone());

    // Unapply is a complete removal, so `~/.fastctx/` is always deleted.
    let settings_action = FileAction::Delete;

    let mut changes = vec![
        FileChange {
            target: paths.codex_config.clone(),
            original: codex_original,
            action: codex_action,
            unix_mode: transaction::existing_unix_mode(&paths.codex_config).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: paths.codex_agents.clone(),
            original: agents_original,
            action: agents_action,
            unix_mode: transaction::existing_unix_mode(&paths.codex_agents).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: paths.fastctx_config.clone(),
            original: settings_original,
            action: settings_action,
            unix_mode: transaction::existing_unix_mode(&paths.fastctx_config).or(Some(0o600)),
            locked_binary_fallback: false,
        },
    ];
    if !running_installed {
        changes.push(FileChange {
            target: paths.installed_binary.clone(),
            original: installed_original,
            action: FileAction::Delete,
            unix_mode: transaction::existing_unix_mode(&paths.installed_binary).or(Some(0o755)),
            locked_binary_fallback: false,
        });
    }
    let preview = preview_unapply(
        &changes,
        restore_token_limit,
        applied
            .as_ref()
            .map(|record| record.tool_output_token_limit),
        applied
            .as_ref()
            .and_then(|record| record.previous_token_limit),
        manual_binary_cleanup.as_deref(),
    );
    Ok(UnapplyPlan {
        changes,
        preview,
        fastctx_dir: paths.fastctx_dir.clone(),
        codex_dir_cleanup,
        manual_binary_cleanup,
        paths: paths.clone(),
        running_jobs,
        running_processes,
    })
}

/// Commits an existing Unapply plan; complete removal always deletes `~/.fastctx/`.
pub fn commit_unapply(plan: UnapplyPlan) -> Result<OperationReceipt, String> {
    let mut admission = crate::shell::jobs::acquire_unapply_admission(&plan.paths)?;
    transaction::validate(&plan.changes)?;
    // Fence older servers while admission is locked so no job can appear after the kill scan.
    admission.advance_generation()?;
    let killed_jobs = crate::shell::jobs::kill_all_running(&plan.paths)?;
    let mut running_processes = plan.running_processes.clone();
    for process in processes::installed_processes(&plan.paths.fastctx_bin_dir).map_err(|error| {
        format!(
            "Cannot refresh running FastCtx processes before removal; no host configuration was changed: {error}"
        )
    })? {
        if process.identity.pid != std::process::id()
            && !running_processes
                .iter()
                .any(|known| known.identity == process.identity)
        {
            running_processes.push(process);
        }
    }
    let mut terminated_processes = 0usize;
    let mut termination_failures = Vec::new();
    for process in &running_processes {
        match processes::terminate_installed_process(process, &plan.paths.fastctx_bin_dir) {
            Ok(TerminationOutcome::Terminated) => terminated_processes += 1,
            Ok(TerminationOutcome::NoLongerManaged) => {}
            Err(error) => termination_failures.push(error),
        }
    }
    if !termination_failures.is_empty() {
        return Err(format!(
            "Cannot terminate every running FastCtx process; no host configuration was changed. Stop the listed processes and retry Unapply: {}",
            termination_failures.join("; ")
        ));
    }
    let changed_targets = plan
        .changes
        .iter()
        .filter(|change| change.is_changed())
        .count();
    transaction::commit(&plan.changes)?;
    let mut notes = Vec::new();
    if let Some(path) = plan.manual_binary_cleanup.as_ref() {
        remove_fastctx_except(&plan.fastctx_dir, path)
            .map_err(|error| augment_directory_error(error, &plan.paths))?;
    } else {
        remove_tree(&plan.fastctx_dir, "FastCtx configuration")
            .map_err(|error| augment_directory_error(error, &plan.paths))?;
    }
    if let Some(directory) = plan.codex_dir_cleanup {
        match fs::remove_dir(&directory) {
            Ok(()) => notes.push(format!(
                "Removed the empty configuration directory {} created by Apply.",
                crate::paths::display_path(&directory)
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                notes.push(format!(
                    "Kept {} because it now contains files not owned by fastctx.",
                    crate::paths::display_path(&directory)
                ));
            }
            Err(error) => notes.push(format!(
                "Could not remove the empty configuration directory {}: {error}. Inspect it and remove it manually if desired.",
                crate::paths::display_path(&directory)
            )),
        }
    }
    if let Some(path) = plan.manual_binary_cleanup {
        notes.push(format!(
            "The running binary could not remove itself. Delete {} after this process exits.",
            crate::paths::display_path(&path)
        ));
    }
    if killed_jobs > 0 {
        notes.push(format!(
            "Stopped {killed_jobs} running background {} before removal.",
            if killed_jobs == 1 { "job" } else { "jobs" }
        ));
    }
    notes.push(format!(
        "Stopped {terminated_processes} running FastCtx {} before removal.",
        if terminated_processes == 1 {
            "process"
        } else {
            "processes"
        }
    ));
    if changed_targets == 0 {
        notes.push("No managed settings were present.".to_string());
    } else {
        notes.push("Changes apply to newly started ChatGPT/Codex sessions.".to_string());
    }
    Ok(OperationReceipt {
        changed_targets,
        notes,
    })
}

fn remove_tree(path: &Path, label: &str) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Cannot remove the {label} directory {}: {error}. Stop processes using it and retry Unapply.",
            crate::paths::display_path(path)
        )),
    }
}

fn augment_directory_error(error: String, paths: &ControlPaths) -> String {
    match processes::installed_processes(&paths.fastctx_bin_dir) {
        Ok(processes) => {
            let processes = processes
                .into_iter()
                .filter(|process| process.identity.pid != std::process::id())
                .collect::<Vec<_>>();
            if processes.is_empty() {
                error
            } else {
                format!(
                    "{error} Remaining managed process images: {}.",
                    processes::process_details(&processes)
                )
            }
        }
        Err(inspect_error) => {
            format!("{error} Remaining process images could not be enumerated: {inspect_error}.")
        }
    }
}

fn remove_fastctx_except(root: &Path, retained: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Cannot inspect FastCtx configuration directory {}: {error}",
                crate::paths::display_path(root)
            ));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Cannot inspect FastCtx configuration directory {}: {error}",
                crate::paths::display_path(root)
            )
        })?;
        let path = entry.path();
        if retained.starts_with(&path) {
            if path.is_dir() {
                remove_directory_except(&path, retained)?;
            }
            continue;
        }
        if path.is_dir() {
            remove_tree(&path, "FastCtx-owned")?;
        } else {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "Cannot remove FastCtx-owned file {}: {error}",
                        crate::paths::display_path(&path)
                    ));
                }
            }
        }
    }
    Ok(())
}

fn remove_directory_except(directory: &Path, retained: &Path) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| {
        format!(
            "Cannot inspect FastCtx-owned directory {}: {error}",
            crate::paths::display_path(directory)
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "Cannot inspect FastCtx-owned directory {}: {error}",
                crate::paths::display_path(directory)
            )
        })?;
        let path = entry.path();
        if path == retained {
            continue;
        }
        if retained.starts_with(&path) && path.is_dir() {
            remove_directory_except(&path, retained)?;
        } else if path.is_dir() {
            remove_tree(&path, "FastCtx-owned")?;
        } else {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Cannot remove FastCtx-owned file {}: {error}",
                    crate::paths::display_path(&path)
                )
            })?;
        }
    }
    Ok(())
}

fn managed_record(
    path: &Path,
    original: &Option<Vec<u8>>,
    applied: &[u8],
    previous: Option<&ManagedFileRecord>,
) -> ManagedFileRecord {
    // Repeated Apply preserves the first original_existed value because the on-disk "original" already contains our block.
    // Looking only at original.is_some() would misclassify an Apply-created file and leave it behind during Unapply.
    let original_existed = previous
        .map(|record| record.original_existed)
        .unwrap_or_else(|| original.is_some());
    ManagedFileRecord {
        path: crate::paths::display_path(path),
        original_existed,
        applied_sha256: sha256(applied),
    }
}

struct RecordMatchContext<'a> {
    expected: &'a ExpectedConfig,
    paths: &'a ControlPaths,
    binary_hash: &'a str,
    agents_bytes: &'a [u8],
    agents_inserted_separator: Option<agents::InsertedSeparator>,
    codex_dir_created: bool,
}

fn record_matches(record: &AppliedRecord, context: &RecordMatchContext<'_>) -> bool {
    let RecordMatchContext {
        expected,
        paths,
        binary_hash,
        agents_bytes,
        agents_inserted_separator,
        codex_dir_created,
    } = context;
    record.version == env!("CARGO_PKG_VERSION")
        && record.command == expected.command
        && record.tier == expected.tier
        && record.tool_budgets == expected.tool_budgets
        && record.fastshell_enabled == expected.fastshell_enabled
        && !record.fastedit_enabled
        && record.tool_output_token_limit == expected.tier.host_limit()
        && record.tool_timeout_sec == Some(codex_config::TOOL_TIMEOUT_SECONDS)
        && record.fastctx_token_budget == expected.tier.fastctx_budget()
        && record.codex_dir_created == *codex_dir_created
        && record.codex_config.path == crate::paths::display_path(&paths.codex_config)
        && record.codex_agents.path == crate::paths::display_path(&paths.codex_agents)
        && record.binary_sha256 == *binary_hash
        && record.codex_agents.applied_sha256 == sha256(agents_bytes)
        && record.codex_agents_inserted_separator == *agents_inserted_separator
}

fn codex_directory_will_be_created(paths: &ControlPaths) -> Result<bool, String> {
    match fs::metadata(&paths.codex_dir) {
        Ok(metadata) if metadata.is_dir() => Ok(false),
        Ok(_) => Err(format!(
            "Cannot create ChatGPT/Codex configuration files because {} is not a directory.",
            crate::paths::display_path(&paths.codex_dir)
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!(
            "Cannot inspect the ChatGPT/Codex configuration directory {}: {error}",
            crate::paths::display_path(&paths.codex_dir)
        )),
    }
}

fn receipt_profile_mismatch(paths: &ControlPaths, record: &AppliedRecord) -> String {
    let recorded_profile = Path::new(&record.codex_config.path)
        .parent()
        .unwrap_or_else(|| Path::new(&record.codex_config.path));
    format!(
        "The selected Codex profile {} (source: {}) does not match the last Apply receipt for {}. Run fastctx unapply --codex-home {} first, then retry; FastCtx will not use ownership evidence from one profile against another.",
        crate::paths::display_path(&paths.codex_dir),
        paths.codex_home_source.as_str(),
        crate::paths::display_path(recorded_profile),
        crate::paths::display_path(recorded_profile),
    )
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

fn is_installed_binary(change: &FileChange) -> bool {
    change.target.file_name().and_then(|name| name.to_str())
        == Some(if cfg!(windows) {
            "fastctx.exe"
        } else {
            "fastctx"
        })
}

fn is_codex_config(change: &FileChange) -> bool {
    change.target.ends_with(".codex/config.toml") || change.target.ends_with(".codex\\config.toml")
}

fn is_codex_agents(change: &FileChange) -> bool {
    change.target.file_name().and_then(|name| name.to_str()) == Some("AGENTS.md")
}

/// Classifies a file change into preview semantics for TUI purpose text.
fn preview_target(change: &FileChange) -> PreviewTarget {
    if is_installed_binary(change) {
        PreviewTarget::Binary
    } else if is_codex_config(change) {
        PreviewTarget::CodexConfig
    } else if is_codex_agents(change) {
        PreviewTarget::Agents
    } else {
        PreviewTarget::Receipt
    }
}

fn short_hash(bytes: &[u8]) -> String {
    sha256(bytes)[..8].to_string()
}

fn budget_env_details(expected: &ExpectedConfig) -> Vec<String> {
    let global = expected.tier.fastctx_budget();
    let mut details = vec![format!("FASTCTX_TOKEN_BUDGET = {global}")];
    for (variable, level) in [
        ("FASTCTX_READ_TOKEN_BUDGET", expected.tool_budgets.read),
        ("FASTCTX_GREP_TOKEN_BUDGET", expected.tool_budgets.grep),
        ("FASTCTX_GLOB_TOKEN_BUDGET", expected.tool_budgets.glob),
        ("FASTCTX_RUN_TOKEN_BUDGET", expected.tool_budgets.run),
        (
            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
            expected.tool_budgets.job_output,
        ),
    ] {
        if let Some(value) = level.resolve(global) {
            details.push(format!("{variable} = {value}"));
        }
    }
    details
}

fn preview_apply(
    changes: &[FileChange],
    expected: &ExpectedConfig,
    previous_token_limit_present: bool,
    previous_token_limit: Option<i64>,
) -> Vec<PreviewItem> {
    changes
        .iter()
        .map(|change| {
            if !change.is_changed() {
                return PreviewItem {
                    path: change.target.clone(),
                    action: PreviewAction::Unchanged,
                    target: preview_target(change),
                    details: Vec::new(),
                };
            }
            let (action, details) = if is_installed_binary(change) {
                let new_hash = match &change.action {
                    FileAction::Write(bytes) => short_hash(bytes),
                    FileAction::Delete => String::new(),
                };
                let hash_line = match change.original.as_deref() {
                    Some(old) => format!("sha256 {} → {new_hash}", short_hash(old)),
                    None => format!("sha256 {new_hash}"),
                };
                (
                    PreviewAction::Install,
                    vec![
                        PreviewDetail::kept(format!("fastctx v{}", env!("CARGO_PKG_VERSION"))),
                        PreviewDetail::kept(hash_line),
                    ],
                )
            } else if is_codex_config(change) {
                let new_limit = expected.tier.host_limit();
                let original = change.original.as_deref().unwrap_or_default();
                let updated = match &change.action {
                    FileAction::Write(bytes) => bytes.as_slice(),
                    FileAction::Delete => &[],
                };
                let mut details = Vec::new();
                for legacy in ["fastread", "fastshell", "fastedit"] {
                    if codex_config::has_server(original, legacy)
                        && !codex_config::has_server(updated, legacy)
                    {
                        details.push(PreviewDetail::removed(format!(
                            "[mcp_servers.{legacy}]"
                        )));
                    }
                }
                for legacy in ["mcp__fastread", "mcp__fastshell", "mcp__fastedit"] {
                    if codex_config::has_namespace(original, legacy)
                        && !codex_config::has_namespace(updated, legacy)
                    {
                        details.push(PreviewDetail::removed(format!(
                            "direct_only_tool_namespaces -= \"{legacy}\""
                        )));
                    }
                }
                details.extend([
                    PreviewDetail::kept(format!(
                        "[mcp_servers.fastctx] command = {}",
                        expected.command
                    )),
                    PreviewDetail::kept(format!(
                        "[mcp_servers.fastctx] args = [{}]",
                        server_args_preview(expected)
                    )),
                    PreviewDetail::kept("[mcp_servers.fastctx] startup_timeout_sec = 120"),
                    PreviewDetail::kept("[mcp_servers.fastctx] tool_timeout_sec = 300"),
                    PreviewDetail::kept("direct_only_tool_namespaces += \"mcp__fastctx\""),
                ]);
                match previous_token_limit {
                    Some(current) if current == new_limit => {}
                    Some(current) => {
                        details.push(PreviewDetail::kept(format!(
                            "tool_output_token_limit {current} → {new_limit}"
                        )));
                    }
                    None if previous_token_limit_present => {
                        details.push(PreviewDetail::kept(format!(
                            "tool_output_token_limit → {new_limit}"
                        )));
                    }
                    None => details.push(PreviewDetail::kept(format!(
                        "tool_output_token_limit (unset) → {new_limit}"
                    ))),
                }
                details.extend(
                    budget_env_details(expected)
                        .into_iter()
                        .map(PreviewDetail::kept),
                );
                (PreviewAction::Modify, details)
            } else if is_codex_agents(change) {
                let original = change.original.as_deref().unwrap_or_default();
                let updated = match &change.action {
                    FileAction::Write(bytes) => bytes.as_slice(),
                    FileAction::Delete => &[],
                };
                let mut details = Vec::new();
                if contains_bytes(original, b"<!-- fastread:begin -->")
                    && !contains_bytes(updated, b"<!-- fastread:begin -->")
                {
                    details.push(PreviewDetail::removed(
                        "<!-- fastread:begin --> … <!-- fastread:end -->",
                    ));
                }
                if contains_bytes(original, b"mcp__fastctx__copy")
                    && !contains_bytes(updated, b"mcp__fastctx__copy")
                {
                    details.push(PreviewDetail::removed(
                        "named clipboard guidance",
                    ));
                }
                details.push(PreviewDetail::kept(
                    "<!-- fastctx:begin --> … <!-- fastctx:end -->",
                ));
                (
                    PreviewAction::Modify,
                    details,
                )
            } else {
                (
                    PreviewAction::Record,
                    vec![PreviewDetail::kept(format!(
                        "tier = {} · read/grep/glob/run/job_output = {}/{}/{}/{}/{} · fastshell = {}",
                        expected.tier.as_str(),
                        expected.tool_budgets.read.as_str(),
                        expected.tool_budgets.grep.as_str(),
                        expected.tool_budgets.glob.as_str(),
                        expected.tool_budgets.run.as_str(),
                        expected.tool_budgets.job_output.as_str(),
                        if expected.fastshell_enabled {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    ))],
                )
            };
            PreviewItem {
                path: change.target.clone(),
                action,
                target: preview_target(change),
                details,
            }
        })
        .collect()
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn preview_unapply(
    changes: &[FileChange],
    restore_token_limit: bool,
    applied_token_limit: Option<i64>,
    previous_token_limit: Option<i64>,
    manual_binary_cleanup: Option<&Path>,
) -> Vec<PreviewItem> {
    let mut preview = changes
        .iter()
        .map(|change| {
            if !change.is_changed() {
                return PreviewItem {
                    path: change.target.clone(),
                    action: PreviewAction::Unchanged,
                    target: preview_target(change),
                    details: Vec::new(),
                };
            }
            let (action, details) = if is_codex_config(change) {
                let original = change.original.as_deref().unwrap_or_default();
                let mut details = Vec::new();
                if codex_config::has_server(original, "fastctx") {
                    details.push(PreviewDetail::removed("[mcp_servers.fastctx]"));
                }
                if codex_config::has_namespace(original, "mcp__fastctx") {
                    details.push(PreviewDetail::removed(
                        "direct_only_tool_namespaces -= \"mcp__fastctx\"",
                    ));
                }
                if restore_token_limit {
                    match (applied_token_limit, previous_token_limit) {
                        // Restoring a different previous value is a modification line (old to new).
                        (Some(applied), Some(previous)) if applied != previous => {
                            details.push(PreviewDetail::kept(format!(
                                "tool_output_token_limit {applied} → {previous}"
                            )));
                        }
                        // When the key was originally absent, restoration deletes it.
                        (_, None) => {
                            details.push(PreviewDetail::removed("tool_output_token_limit"));
                        }
                        // When the old value equals the applied value, Apply changed nothing and no detail is needed.
                        _ => {}
                    }
                }
                (PreviewAction::Modify, details)
            } else if is_codex_agents(change) {
                (
                    match change.action {
                        FileAction::Delete => PreviewAction::Delete,
                        FileAction::Write(_) => PreviewAction::Modify,
                    },
                    vec![PreviewDetail::removed(
                        "<!-- fastctx:begin --> … <!-- fastctx:end -->",
                    )],
                )
            } else {
                (
                    match change.action {
                        FileAction::Delete => PreviewAction::Delete,
                        FileAction::Write(_) => PreviewAction::Record,
                    },
                    vec![PreviewDetail::removed("applied receipt")],
                )
            };
            PreviewItem {
                path: change.target.clone(),
                action,
                target: preview_target(change),
                details,
            }
        })
        .collect::<Vec<_>>();
    if let Some(path) = manual_binary_cleanup {
        preview.push(PreviewItem {
            path: path.to_path_buf(),
            action: PreviewAction::Keep,
            target: PreviewTarget::Binary,
            details: Vec::new(),
        });
    }
    preview
}

fn server_args_preview(expected: &ExpectedConfig) -> String {
    let mut args = vec!["\"serve\""];
    if expected.fastshell_enabled {
        args.push("\"--enable-shell\"");
    }
    args.join(", ")
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

fn find_stale_binaries(
    paths: &ControlPaths,
    current_executable: &Path,
) -> Result<Vec<PathBuf>, String> {
    let mut stale = crate::control::leftovers::stale_binary_siblings(&paths.installed_binary)?;
    if !same_path(current_executable, &paths.installed_binary) {
        stale.extend(crate::control::leftovers::stale_binary_siblings(
            current_executable,
        )?);
    }
    stale.sort();
    stale.dedup();
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::{
        AppliedBinarySync, ApplyOptions, PreviewAction, PreviewTarget, UnapplyOptions,
        commit_apply, commit_unapply, plan_apply, plan_unapply, synchronize_applied_binary,
    };
    use crate::control::agents::AGENTS_SECTION;
    use crate::control::paths::ControlPaths;
    use crate::control::settings::{Tier, ToolBudgetLevel, ToolBudgets};
    use std::path::Path;

    fn fixture() -> (tempfile::TempDir, ControlPaths, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        std::fs::create_dir_all(&paths.codex_dir).unwrap();
        let executable = temp.path().join(if cfg!(windows) {
            "source-fastctx.exe"
        } else {
            "source-fastctx"
        });
        std::fs::write(&executable, b"binary fixture").unwrap();
        (temp, paths, executable)
    }

    fn spawn_managed_fixture(paths: &ControlPaths) -> std::process::Child {
        std::fs::create_dir_all(&paths.fastctx_bin_dir).unwrap();
        let target = paths.fastctx_bin_dir.join(if cfg!(windows) {
            "managed-fixture.exe"
        } else {
            "managed-fixture"
        });
        #[cfg(windows)]
        {
            let source = std::env::var_os("ComSpec")
                .unwrap_or_else(|| r"C:\Windows\System32\cmd.exe".into());
            std::fs::copy(source, &target).unwrap();
            std::process::Command::new(target)
                .args(["/D", "/Q", "/C", "ping -n 30 127.0.0.1 >NUL"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::PermissionsExt;

            // macOS may report a copied system shell by its protected source image. A copied test
            // executable exercises the same launch shape as the installed FastCtx binary.
            std::fs::copy(std::env::current_exe().unwrap(), &target).unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::process::Command::new(target)
                .args([
                    "--ignored",
                    "--exact",
                    "control::apply::tests::managed_process_fixture",
                ])
                .env("FASTCTX_MANAGED_PROCESS_FIXTURE", "1")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            crate::control::processes::publish_unix_executable_fixture(
                std::path::Path::new("/bin/sh"),
                &target,
            )
            .unwrap();
            std::process::Command::new(target)
                .args(["-c", "while :; do sleep 1; done"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        }
    }

    #[test]
    #[ignore]
    fn managed_process_fixture() {
        if std::env::var("FASTCTX_MANAGED_PROCESS_FIXTURE")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        loop {
            std::thread::park_timeout(std::time::Duration::from_secs(60));
        }
    }

    fn wait_for_managed_fixture(
        child: &mut std::process::Child,
        paths: &ControlPaths,
    ) -> crate::control::processes::InstalledProcess {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let processes =
                crate::control::processes::installed_processes(&paths.fastctx_bin_dir).unwrap();
            if let Some(process) = processes
                .into_iter()
                .find(|process| process.identity.pid == child.id())
            {
                return process;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "managed fixture PID {} exited before discovery: {status}",
                    child.id()
                );
            }
            if std::time::Instant::now() >= deadline {
                let pid = child.id();
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "managed fixture PID {pid} was not discovered below {}",
                    crate::paths::display_path(&paths.fastctx_bin_dir)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    fn options(executable: std::path::PathBuf) -> ApplyOptions {
        ApplyOptions {
            tier: Tier::Standard,
            tool_budgets: ToolBudgets {
                read: ToolBudgetLevel::Inherit,
                grep: ToolBudgetLevel::Percent50,
                glob: ToolBudgetLevel::Percent25,
                run: ToolBudgetLevel::Inherit,
                job_output: ToolBudgetLevel::Inherit,
            },
            fastshell_enabled: false,
            current_executable: executable,
        }
    }

    #[test]
    fn apply_is_idempotent_and_unapply_restores_user_bytes() {
        let (_temp, paths, executable) = fixture();
        let config = concat!(
            "# user config\n",
            "theme = 'dark'\n",
            "\n",
            "[mcp_servers.other]\n",
            "command = 'other'\n",
            "\n",
            "[features.code_mode]\n",
            "direct_only_tool_namespaces = [ 'other' ]\n",
        );
        let agents = "# User rules\n\nKeep this exact.\n";
        std::fs::write(&paths.codex_config, config).unwrap();
        std::fs::write(&paths.codex_agents, agents).unwrap();

        let first = plan_apply(&paths, options(executable.clone())).unwrap();
        assert!(!first.is_empty());
        commit_apply(first, true).unwrap();
        let applied_config = std::fs::read(&paths.codex_config).unwrap();
        assert!(
            std::str::from_utf8(&applied_config)
                .unwrap()
                .contains("mcp__fastctx")
        );

        let second = plan_apply(&paths, options(executable.clone())).unwrap();
        assert!(second.is_empty(), "{:?}", second.preview());
        assert_eq!(commit_apply(second, true).unwrap().changed_targets, 0);

        let unapply = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        commit_unapply(unapply).unwrap();
        assert_eq!(
            std::fs::read(&paths.codex_config).unwrap(),
            config.as_bytes()
        );
        assert_eq!(
            std::fs::read(&paths.codex_agents).unwrap(),
            agents.as_bytes()
        );
        assert!(!paths.installed_binary.exists());
        assert!(!paths.fastctx_dir.exists());
    }

    #[test]
    fn unapply_terminates_real_managed_process_before_removing_the_private_tree() {
        let (_temp, paths, executable) = fixture();
        let mut child = spawn_managed_fixture(&paths);
        let discovered = wait_for_managed_fixture(&mut child, &paths);
        assert_eq!(discovered.identity.pid, child.id());

        let plan = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        assert_eq!(plan.running_processes(), 1);
        assert!(!plan.is_empty());
        let receipt = commit_unapply(plan).unwrap();

        assert!(
            child.try_wait().unwrap().is_some(),
            "Unapply must wait for the managed process to exit"
        );
        assert!(!paths.fastctx_dir.exists());
        assert!(
            receipt
                .notes
                .iter()
                .any(|note| note == "Stopped 1 running FastCtx process before removal."),
            "{:?}",
            receipt.notes
        );
    }

    #[test]
    fn unapply_commit_catches_a_managed_process_started_after_preview() {
        let (_temp, paths, executable) = fixture();
        std::fs::create_dir_all(&paths.fastctx_dir).unwrap();
        let plan = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        assert_eq!(plan.running_processes(), 0);

        let mut child = spawn_managed_fixture(&paths);
        let discovered = wait_for_managed_fixture(&mut child, &paths);
        assert_eq!(discovered.identity.pid, child.id());
        let receipt = commit_unapply(plan).unwrap();
        assert!(child.try_wait().unwrap().is_some());
        assert!(!paths.fastctx_dir.exists());
        assert!(
            receipt
                .notes
                .iter()
                .any(|note| note == "Stopped 1 running FastCtx process before removal."),
            "{:?}",
            receipt.notes
        );
    }

    #[test]
    fn explicit_product_update_advances_only_an_owned_stable_binary_and_receipt() {
        let (_temp, paths, executable) = fixture();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        std::fs::write(&executable, b"new signed release bytes").unwrap();

        assert_eq!(
            synchronize_applied_binary(&paths, &executable).unwrap(),
            AppliedBinarySync::Updated
        );
        assert_eq!(
            std::fs::read(&paths.installed_binary).unwrap(),
            b"new signed release bytes"
        );
        let settings = crate::control::settings::load(&paths).unwrap();
        let receipt = settings.applied.unwrap();
        assert_eq!(receipt.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            receipt.binary_sha256,
            super::sha256(b"new signed release bytes")
        );

        std::fs::write(&paths.installed_binary, b"user replacement").unwrap();
        std::fs::write(&executable, b"another release").unwrap();
        let error = synchronize_applied_binary(&paths, &executable).unwrap_err();
        assert!(error.contains("changed outside FastCtx"), "{error}");
        assert_eq!(
            std::fs::read(&paths.installed_binary).unwrap(),
            b"user replacement"
        );
    }

    #[test]
    fn reapply_ignores_host_owned_codex_config_rewrites() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_config, b"# user config\n").unwrap();
        std::fs::write(&paths.codex_agents, b"# user rules\n").unwrap();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();

        let settings_before = std::fs::read(&paths.fastctx_config).unwrap();
        let mut host_rewritten = std::fs::read(&paths.codex_config).unwrap();
        host_rewritten
            .extend_from_slice(b"\n[plugins.runtime]\nlast_refresh = \"2026-07-17T00:01:00Z\"\n");
        std::fs::write(&paths.codex_config, &host_rewritten).unwrap();

        let second = plan_apply(&paths, options(executable)).unwrap();
        assert!(second.is_empty(), "{:?}", second.preview());
        assert_eq!(commit_apply(second, true).unwrap().changed_targets, 0);
        assert_eq!(std::fs::read(&paths.codex_config).unwrap(), host_rewritten);
        assert_eq!(
            std::fs::read(&paths.fastctx_config).unwrap(),
            settings_before
        );
    }

    #[test]
    fn stale_unapply_preview_is_rejected_before_admission_or_cleanup_changes() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_config, b"# user config\n").unwrap();
        std::fs::write(&paths.codex_agents, b"# user rules\n").unwrap();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let plan = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        let generation_before = crate::shell::jobs::admission::observe_generation(&paths).unwrap();
        let mut drifted = std::fs::read(&paths.codex_config).unwrap();
        drifted.extend_from_slice(b"\n# concurrent user edit\n");
        std::fs::write(&paths.codex_config, &drifted).unwrap();

        let error = commit_unapply(plan).unwrap_err();
        assert!(error.contains("changed after the preview"), "{error}");
        assert_eq!(std::fs::read(&paths.codex_config).unwrap(), drifted);
        assert!(paths.fastctx_config.exists());
        assert!(paths.installed_binary.exists());
        assert_eq!(
            crate::shell::jobs::admission::observe_generation(&paths).unwrap(),
            generation_before
        );
    }

    #[test]
    fn changing_tier_then_unapply_restores_user_bytes_via_reverse_edits() {
        // Critical regression: when Apply created mcp_servers/features, Unapply must reverse it byte-for-byte,
        // including empty parent cleanup, without relying on backups (2026-07-12).
        let (_temp, paths, executable) = fixture();
        let config = b"# user\ntool_output_token_limit = 9000\n";
        let agents = b"# rules\n";
        std::fs::write(&paths.codex_config, config).unwrap();
        std::fs::write(&paths.codex_agents, agents).unwrap();

        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let mut changed = options(executable.clone());
        changed.tier = Tier::High;
        commit_apply(plan_apply(&paths, changed).unwrap(), true).unwrap();

        let unapply = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        commit_unapply(unapply).unwrap();
        assert_eq!(std::fs::read(&paths.codex_config).unwrap(), config);
        assert_eq!(std::fs::read(&paths.codex_agents).unwrap(), agents);
    }

    #[test]
    fn agents_apply_unapply_roundtrip_preserves_every_supported_file_ending() {
        let cases: &[(&str, &[u8])] = &[
            ("empty file", b""),
            ("no trailing newline", b"# rules"),
            ("one trailing LF", b"# rules\n"),
            ("existing LF blank line", b"# rules\n\n"),
            ("one trailing CRLF", b"# rules\r\n"),
            ("existing CRLF blank line", b"# rules\r\n\r\n"),
        ];

        for (name, original) in cases {
            let (_temp, paths, executable) = fixture();
            std::fs::write(&paths.codex_config, b"# config\n").unwrap();
            std::fs::write(&paths.codex_agents, original).unwrap();

            commit_apply(
                plan_apply(&paths, options(executable.clone())).unwrap(),
                true,
            )
            .unwrap();
            let applied_bytes = std::fs::read(&paths.codex_agents).unwrap();
            assert_ne!(applied_bytes, *original, "{name}");

            let second = plan_apply(&paths, options(executable.clone())).unwrap();
            assert!(second.is_empty(), "{name}: {:?}", second.preview());
            commit_apply(second, true).unwrap();

            commit_unapply(
                plan_unapply(
                    &paths,
                    UnapplyOptions {
                        current_executable: executable,
                    },
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(&paths.codex_agents).unwrap(),
                *original,
                "{name}"
            );
        }
    }

    #[test]
    fn legacy_receipt_without_separator_ownership_does_not_guess_at_user_bytes() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_config, b"# config\n").unwrap();
        std::fs::write(&paths.codex_agents, b"# rules\n").unwrap();

        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let settings = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(
            settings.contains("codex_agents_inserted_separator = \"lf\""),
            "{settings}"
        );
        let legacy_settings = settings.replace("codex_agents_inserted_separator = \"lf\"\n", "");
        std::fs::write(&paths.fastctx_config, legacy_settings).unwrap();

        commit_unapply(
            plan_unapply(
                &paths,
                UnapplyOptions {
                    current_executable: executable,
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&paths.codex_agents).unwrap(), b"# rules\n\n");
    }

    #[test]
    fn legacy_receipt_without_directory_ownership_preserves_a_preexisting_empty_profile() {
        let (_temp, paths, executable) = fixture();

        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let settings = std::fs::read_to_string(&paths.fastctx_config).unwrap();
        assert!(
            settings.contains("codex_dir_created = false\n"),
            "{settings}"
        );
        let legacy_settings = settings.replace("codex_dir_created = false\n", "");
        std::fs::write(&paths.fastctx_config, legacy_settings).unwrap();

        commit_unapply(
            plan_unapply(
                &paths,
                UnapplyOptions {
                    current_executable: executable,
                },
            )
            .unwrap(),
        )
        .unwrap();

        assert!(paths.codex_dir.is_dir());
        assert!(
            std::fs::read_dir(&paths.codex_dir)
                .unwrap()
                .next()
                .is_none()
        );
        assert!(!paths.fastctx_dir.exists());
    }

    #[test]
    fn reapply_after_user_drift_keeps_later_edits_during_unapply() {
        let (_temp, paths, executable) = fixture();
        let config = b"# original\ntool_output_token_limit = 9000\n";
        let agents = b"# original rules\n";
        std::fs::write(&paths.codex_config, config).unwrap();
        std::fs::write(&paths.codex_agents, agents).unwrap();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();

        let mut drifted_config = std::fs::read_to_string(&paths.codex_config).unwrap();
        drifted_config.push_str("\n[user_after_apply]\nkept = true\n");
        std::fs::write(&paths.codex_config, drifted_config).unwrap();
        let mut drifted_agents = std::fs::read_to_string(&paths.codex_agents).unwrap();
        drifted_agents.push_str("\nKeep this later rule.\n");
        std::fs::write(&paths.codex_agents, drifted_agents).unwrap();

        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let mut later_fastctx_change = options(executable.clone());
        later_fastctx_change.tier = Tier::High;
        commit_apply(plan_apply(&paths, later_fastctx_change).unwrap(), true).unwrap();

        let unapply = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        commit_unapply(unapply).unwrap();
        let restored_config = std::fs::read_to_string(&paths.codex_config).unwrap();
        assert!(!restored_config.contains("mcp_servers.fastctx"));
        assert!(!restored_config.contains("mcp__fastctx"));
        assert!(restored_config.contains("[user_after_apply]\nkept = true"));
        assert!(restored_config.contains("tool_output_token_limit = 9000"));
        let restored_agents = std::fs::read_to_string(&paths.codex_agents).unwrap();
        assert!(!restored_agents.contains("<!-- fastctx:begin -->"));
        assert!(restored_agents.contains("Keep this later rule."));
    }

    #[test]
    fn a_shared_token_limit_conflict_needs_explicit_confirmation() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_config, b"tool_output_token_limit = 9000\n").unwrap();
        let plan = plan_apply(&paths, options(executable)).unwrap();
        let conflict = plan.token_limit_conflict().unwrap();
        assert_eq!((conflict.current, conflict.requested), (9_000, 16_000));
        let error = commit_apply(plan, false).unwrap_err();
        assert!(error.contains("Re-run with --yes"));
        assert_eq!(
            std::fs::read(&paths.codex_config).unwrap(),
            b"tool_output_token_limit = 9000\n"
        );
    }

    #[test]
    fn fresh_environment_creates_profile_files_and_unapply_removes_the_owned_empty_shell() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("fastctx-source");
        std::fs::write(&executable, b"binary").unwrap();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();

        assert!(paths.codex_dir.is_dir());
        assert!(paths.codex_config.is_file());
        assert!(paths.codex_agents.is_file());
        let saved = crate::control::settings::load(&paths).unwrap();
        assert!(saved.applied.as_ref().unwrap().codex_dir_created);

        commit_unapply(
            plan_unapply(
                &paths,
                UnapplyOptions {
                    current_executable: executable,
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert!(!paths.codex_config.exists());
        assert!(!paths.codex_agents.exists());
        assert!(!paths.codex_dir.exists());
        assert!(!paths.fastctx_dir.exists());
    }

    #[test]
    fn unapply_keeps_a_created_codex_directory_after_the_user_adds_content() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("fastctx-source");
        std::fs::write(&executable, b"binary").unwrap();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let user_file = paths.codex_dir.join("user-owned.toml");
        std::fs::write(&user_file, b"kept = true\n").unwrap();

        commit_unapply(
            plan_unapply(
                &paths,
                UnapplyOptions {
                    current_executable: executable,
                },
            )
            .unwrap(),
        )
        .unwrap();

        assert!(paths.codex_dir.is_dir());
        assert_eq!(std::fs::read(&user_file).unwrap(), b"kept = true\n");
        assert!(!paths.codex_config.exists());
        assert!(!paths.codex_agents.exists());
    }

    #[test]
    fn apply_creates_a_missing_codex_config_with_only_the_managed_shape() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_agents, b"# existing rules\n").unwrap();

        commit_apply(plan_apply(&paths, options(executable)).unwrap(), true).unwrap();

        let source = std::fs::read_to_string(&paths.codex_config).unwrap();
        let document = source.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(document.iter().count(), 3, "{source}");
        assert_eq!(
            document
                .get("tool_output_token_limit")
                .and_then(toml_edit::Item::as_integer),
            Some(16_000)
        );
        assert!(source.contains("[mcp_servers.fastctx]"), "{source}");
        assert!(source.contains("mcp__fastctx"), "{source}");
        assert!(source.contains("FASTCTX_TOKEN_BUDGET = \"13600\""));
    }

    #[test]
    fn apply_creates_a_missing_agents_file_with_the_exact_private_block() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_config, b"# existing config\n").unwrap();

        commit_apply(plan_apply(&paths, options(executable)).unwrap(), true).unwrap();

        let mut expected = AGENTS_SECTION.as_bytes().to_vec();
        expected.push(b'\n');
        assert_eq!(std::fs::read(&paths.codex_agents).unwrap(), expected);
    }

    #[test]
    fn malformed_codex_toml_aborts_before_self_install_or_settings_creation() {
        let (_temp, paths, executable) = fixture();
        std::fs::write(&paths.codex_config, b"[broken").unwrap();
        let error = plan_apply(&paths, options(executable)).unwrap_err();
        assert!(error.contains("Cannot parse Codex config.toml"));
        assert!(!paths.installed_binary.exists());
        assert!(!paths.fastctx_config.exists());
        assert_eq!(std::fs::read(&paths.codex_config).unwrap(), b"[broken");
    }

    #[test]
    fn a_non_directory_codex_profile_aborts_before_any_write() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join("fastctx-source");
        std::fs::write(&executable, b"binary").unwrap();
        std::fs::write(&paths.codex_dir, b"user-owned path").unwrap();

        let error = plan_apply(&paths, options(executable)).unwrap_err();

        assert!(error.contains("is not a directory"), "{error}");
        assert_eq!(std::fs::read(&paths.codex_dir).unwrap(), b"user-owned path");
        assert!(!paths.fastctx_dir.exists());
    }

    #[test]
    fn unapply_leaves_a_user_edited_token_limit_untouched() {
        // Ownership-aware regression: after the user changes the shared token key, ownership returns to the user.
        // Unapply removes FastCtx-owned blocks but does not restore the user-modified shared key (2026-07-12).
        let (_temp, paths, executable) = fixture();
        let config = b"# user\ntool_output_token_limit = 9000\n";
        std::fs::write(&paths.codex_config, config).unwrap();
        std::fs::write(&paths.codex_agents, b"# rules\n").unwrap();
        commit_apply(
            plan_apply(&paths, options(executable.clone())).unwrap(),
            true,
        )
        .unwrap();
        let applied = std::fs::read_to_string(&paths.codex_config).unwrap();
        assert!(applied.contains("tool_output_token_limit = 16000"));
        let edited = applied.replace(
            "tool_output_token_limit = 16000",
            "tool_output_token_limit = 40000",
        );
        std::fs::write(&paths.codex_config, &edited).unwrap();

        let unapply = plan_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable,
            },
        )
        .unwrap();
        commit_unapply(unapply).unwrap();
        let restored = std::fs::read_to_string(&paths.codex_config).unwrap();
        assert!(!restored.contains("mcp__fastctx"), "{restored}");
        assert!(
            restored.contains("tool_output_token_limit = 40000"),
            "{restored}"
        );
        assert!(
            !restored.contains("tool_output_token_limit = 9000"),
            "{restored}"
        );
    }

    #[test]
    fn read_only_codex_config_fails_explicitly_before_any_write() {
        let (_temp, paths, executable) = fixture();
        let config = b"# read only\n";
        std::fs::write(&paths.codex_config, config).unwrap();
        let plan = plan_apply(&paths, options(executable)).unwrap();
        let original_permissions = std::fs::metadata(&paths.codex_config)
            .unwrap()
            .permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&paths.codex_config, permissions).unwrap();

        let result = commit_apply(plan, true);
        std::fs::set_permissions(&paths.codex_config, original_permissions).unwrap();
        let error = result.unwrap_err();
        assert!(error.contains("read-only file"), "{error}");
        assert!(error.contains("Make it writable and retry"), "{error}");
        assert_eq!(std::fs::read(&paths.codex_config).unwrap(), config);
        assert!(!paths.installed_binary.exists());
        assert!(!paths.fastctx_config.exists());
    }

    #[test]
    fn apply_previews_and_removes_leftovers_beside_both_running_and_stable_binaries() {
        let (_temp, paths, source) = fixture();
        std::fs::create_dir_all(&paths.fastctx_bin_dir).unwrap();
        let owned_leftovers = |target: &Path| {
            let name = target.file_name().unwrap().to_string_lossy();
            [
                target
                    .parent()
                    .unwrap()
                    .join(format!(".{name}.fastctx-old-12.0")),
                target.parent().unwrap().join(format!("{name}~RF1a2B.TMP")),
            ]
        };
        let mut stale = owned_leftovers(&source).into_iter().collect::<Vec<_>>();
        stale.extend(owned_leftovers(&paths.installed_binary));
        for path in &stale {
            std::fs::write(path, b"owned stale binary").unwrap();
        }
        let unrelated = source.parent().unwrap().join("other~RF1a2B.TMP");
        std::fs::write(&unrelated, b"user file").unwrap();

        let plan = plan_apply(&paths, options(source)).unwrap();
        for path in &stale {
            assert!(
                plan.preview().iter().any(|item| {
                    item.path == *path
                        && item.action == PreviewAction::Delete
                        && item.target == PreviewTarget::Binary
                }),
                "missing cleanup preview for {}",
                crate::paths::display_path(path)
            );
        }
        let receipt = commit_apply(plan, true).unwrap();

        assert!(receipt.changed_targets >= stale.len());
        for path in stale {
            assert!(!path.exists(), "{}", crate::paths::display_path(&path));
        }
        assert_eq!(std::fs::read(unrelated).unwrap(), b"user file");
    }

    #[cfg(windows)]
    #[test]
    fn running_windows_binary_is_renamed_aside_and_replaced() {
        use std::process::{Command, Stdio};

        let (_temp, paths, source) = fixture();
        std::fs::create_dir_all(&paths.fastctx_bin_dir).unwrap();
        let system_root = std::env::var_os("SystemRoot").unwrap();
        let command_processor = std::path::PathBuf::from(system_root).join("System32/cmd.exe");
        std::fs::copy(&command_processor, &paths.installed_binary).unwrap();
        let mut child = Command::new(&paths.installed_binary)
            .args(["/d", "/q", "/c", "set /p hold="])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));

        let plan = plan_apply(&paths, options(source.clone())).unwrap();
        let receipt = commit_apply(plan, true).unwrap();
        assert!(receipt.changed_targets >= 3);
        assert_eq!(
            std::fs::read(&paths.installed_binary).unwrap(),
            b"binary fixture"
        );
        let _ = child.kill();
        let _ = child.wait();
        let cleanup = plan_apply(&paths, options(source)).unwrap();
        commit_apply(cleanup, true).unwrap();
        let stale = std::fs::read_dir(&paths.fastctx_bin_dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("fastctx-old"));
        assert!(!stale);
    }
}
