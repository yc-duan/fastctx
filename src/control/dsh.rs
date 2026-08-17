//! DeepSeek Harness host integration.

use crate::control::agents;
use crate::control::dsh_config::{self, BlockState, ExpectedConfig};
use crate::control::paths::ControlPaths;
use crate::control::settings::{
    self, DshAppliedRecord, InstallationRecord, ManagedFileRecord, Tier, ToolBudgetPreferences,
};
use crate::control::transaction::{self, FileAction, FileChange};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::format_description;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyOptions {
    pub tier: Tier,
    pub tool_budgets: ToolBudgetPreferences,
    pub fastshell_enabled: bool,
    pub current_executable: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub changes: Vec<FileChange>,
    pub patch_state: BlockState,
    pub dsh_home: PathBuf,
    pub binary_changed: bool,
    pub complete_removal: bool,
    pub complete_plan: Option<Box<crate::control::apply::UnapplyPlan>>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.complete_plan
            .as_deref()
            .is_some_and(crate::control::apply::UnapplyPlan::is_empty)
            || (self.complete_plan.is_none()
                && self.changes.iter().all(|change| !change.is_changed()))
    }

    pub fn preview_changes(&self) -> &[FileChange] {
        self.complete_plan
            .as_deref()
            .map_or(&self.changes, crate::control::apply::UnapplyPlan::changes)
    }

    pub fn running_jobs(&self) -> usize {
        self.complete_plan
            .as_deref()
            .map_or(0, |plan| plan.running_jobs())
    }

    pub fn running_processes(&self) -> usize {
        self.complete_plan
            .as_deref()
            .map_or(0, |plan| plan.running_processes())
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn timestamp() -> Result<String, String> {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second].[subsecond digits:9]Z"
        ))
        .map_err(|error| format!("Cannot format the Apply timestamp: {error}"))
}

fn expected(paths: &ControlPaths, options: &ApplyOptions) -> ExpectedConfig {
    ExpectedConfig {
        command: crate::paths::display_path(&paths.installed_binary),
        args: if options.fastshell_enabled {
            vec!["serve".to_string(), "--enable-shell".to_string()]
        } else {
            vec!["serve".to_string()]
        },
        tier: options.tier,
        fastctx_budget: options.tier.fastctx_budget(),
        tool_budgets: options.tool_budgets.resolve(options.tier),
    }
}

fn file_write(
    target: PathBuf,
    original: Option<Vec<u8>>,
    bytes: Vec<u8>,
    mode: Option<u32>,
) -> FileChange {
    FileChange {
        target,
        original,
        action: FileAction::Write(bytes),
        unix_mode: mode,
        locked_binary_fallback: false,
    }
}

fn managed_record(
    path: &Path,
    original: &Option<Vec<u8>>,
    applied: &[u8],
    previous: Option<&ManagedFileRecord>,
) -> ManagedFileRecord {
    ManagedFileRecord {
        path: crate::paths::display_path(path),
        original_existed: previous.map_or(original.is_some(), |record| record.original_existed),
        applied_sha256: sha256(applied),
    }
}

pub fn plan_apply(paths: &ControlPaths, options: ApplyOptions) -> Result<Plan, String> {
    let source_binary = fs::read(&options.current_executable).map_err(|error| {
        format!(
            "Cannot read the running fastctx binary {}: {error}",
            crate::paths::display_path(&options.current_executable)
        )
    })?;
    let binary_hash = sha256(&source_binary);
    let patch_original = transaction::read_snapshot(&paths.dsh_patch)?;
    let patch_source = patch_original.as_deref().unwrap_or_default();
    let expected = expected(paths, &options);
    let patch_edit = dsh_config::apply(patch_source, &expected)?;
    let agents_original = transaction::read_snapshot(&paths.dsh_agents)?;
    let agents_edit = agents::apply_section_with_ownership_for(
        agents_original.as_deref().unwrap_or_default(),
        options.fastshell_enabled,
    )?;
    let installed_original = if options.current_executable == paths.installed_binary {
        Some(source_binary.clone())
    } else {
        transaction::read_snapshot(&paths.installed_binary)?
    };
    let settings_original = transaction::read_snapshot(&paths.fastctx_config)?;
    let mut saved = settings::load(paths)?;
    let previous = saved.integrations.deepseek_harness.clone();
    if let Some(previous) = previous.as_ref() {
        if !same_path(Path::new(&previous.dsh_dir), &paths.dsh_dir) {
            return Err(format!(
                "The selected DSH home {} does not match the last Apply receipt {}.",
                crate::paths::display_path(&paths.dsh_dir),
                previous.dsh_dir
            ));
        }
        let previous_expected = ExpectedConfig {
            command: previous.command.clone(),
            args: previous.args.clone(),
            tier: previous.tier,
            fastctx_budget: previous.fastctx_token_budget,
            tool_budgets: previous.tool_budgets,
        };
        match dsh_config::classify(patch_source, &previous_expected)? {
            BlockState::Current => {}
            state => {
                return Err(format!(
                    "DeepSeek Harness FastCtx patch is {state:?}; refusing to overwrite user changes. Restore the last applied block or disconnect it manually."
                ));
            }
        }
        match agents::classify_managed_section(
            agents_original.as_deref().unwrap_or_default(),
            previous.fastshell_enabled,
        ) {
            agents::ManagedSectionState::Current | agents::ManagedSectionState::KnownLegacy => {}
            state => {
                return Err(format!(
                    "DeepSeek Harness AGENTS.md FastCtx guidance is {state:?}; refusing to overwrite user changes."
                ));
            }
        }
    }
    let applied_at_utc = previous
        .as_ref()
        .filter(|record| {
            record.version == env!("CARGO_PKG_VERSION")
                && record.command == expected.command
                && record.args == expected.args
                && record.tier == options.tier
                && record.fastctx_token_budget == expected.fastctx_budget
                && record.tool_budgets == expected.tool_budgets
                && record.fastshell_enabled == options.fastshell_enabled
                && record.dsh_dir == crate::paths::display_path(&paths.dsh_dir)
                && record.patch.applied_sha256 == sha256(&patch_edit.bytes)
                && record.agents.applied_sha256 == sha256(&agents_edit.bytes)
                && record.agents_contract_id.as_deref() == Some(agents::MANAGED_SECTION_CONTRACT_ID)
        })
        .map(|record| record.applied_at_utc.clone())
        .unwrap_or(timestamp()?);
    saved.tier = options.tier;
    saved.tool_budgets = options.tool_budgets;
    saved.fastshell.enabled = options.fastshell_enabled;
    saved.integrations.deepseek_harness = Some(DshAppliedRecord {
        applied_at_utc,
        version: env!("CARGO_PKG_VERSION").to_string(),
        command: expected.command.clone(),
        args: expected.args.clone(),
        tier: options.tier,
        fastctx_token_budget: expected.fastctx_budget,
        tool_budgets: expected.tool_budgets,
        fastshell_enabled: options.fastshell_enabled,
        dsh_dir: crate::paths::display_path(&paths.dsh_dir),
        patch: managed_record(
            &paths.dsh_patch,
            &patch_original,
            &patch_edit.bytes,
            previous.as_ref().map(|r| &r.patch),
        ),
        agents: managed_record(
            &paths.dsh_agents,
            &agents_original,
            &agents_edit.bytes,
            previous.as_ref().map(|r| &r.agents),
        ),
        agents_contract_id: Some(agents::MANAGED_SECTION_CONTRACT_ID.to_string()),
        agents_inserted_separator: previous
            .as_ref()
            .and_then(|record| record.agents_inserted_separator)
            .or(agents_edit.inserted_separator),
        patch_inserted_separator: previous
            .as_ref()
            .and_then(|record| record.patch_inserted_separator)
            .or(patch_edit.inserted_separator),
    });
    saved.installation = Some(InstallationRecord {
        version: env!("CARGO_PKG_VERSION").to_string(),
        binary_path: expected.command.clone(),
        binary_sha256: binary_hash.clone(),
    });
    if let Some(codex) = saved.integrations.codex.as_mut() {
        if codex.command != expected.command {
            return Err(
                "The Codex and DeepSeek Harness receipts reference different stable binaries. Re-apply Codex before connecting DeepSeek Harness."
                    .to_string(),
            );
        }
        codex.version = env!("CARGO_PKG_VERSION").to_string();
        codex.binary_sha256 = binary_hash;
    }
    let settings_bytes = settings::encode(&saved)?;
    let changes = vec![
        file_write(
            paths.installed_binary.clone(),
            installed_original,
            source_binary,
            Some(0o755),
        ),
        file_write(
            paths.dsh_patch.clone(),
            patch_original,
            patch_edit.bytes,
            transaction::existing_unix_mode(&paths.dsh_patch).or(Some(0o600)),
        ),
        file_write(
            paths.dsh_agents.clone(),
            agents_original,
            agents_edit.bytes,
            transaction::existing_unix_mode(&paths.dsh_agents).or(Some(0o600)),
        ),
        file_write(
            paths.fastctx_config.clone(),
            settings_original,
            settings_bytes,
            transaction::existing_unix_mode(&paths.fastctx_config).or(Some(0o600)),
        ),
    ];
    Ok(Plan {
        binary_changed: changes[0].is_changed(),
        changes,
        patch_state: patch_edit.state,
        dsh_home: paths.dsh_dir.clone(),
        complete_removal: false,
        complete_plan: None,
    })
}

pub fn commit_apply(plan: Plan) -> Result<usize, String> {
    let changed = plan
        .changes
        .iter()
        .filter(|change| change.is_changed())
        .count();
    transaction::commit(&plan.changes)?;
    Ok(changed)
}

pub fn plan_unapply(paths: &ControlPaths, current_executable: PathBuf) -> Result<Plan, String> {
    let settings_original = transaction::read_snapshot(&paths.fastctx_config)?;
    let saved = settings::load(paths)?;
    let Some(receipt) = saved.integrations.deepseek_harness.as_ref() else {
        return Ok(Plan {
            changes: Vec::new(),
            patch_state: BlockState::Missing,
            dsh_home: paths.dsh_dir.clone(),
            binary_changed: false,
            complete_removal: false,
            complete_plan: None,
        });
    };
    if !same_path(Path::new(&receipt.dsh_dir), &paths.dsh_dir) {
        return Err(format!(
            "The selected DSH home {} does not match the last Apply receipt {}.",
            crate::paths::display_path(&paths.dsh_dir),
            receipt.dsh_dir
        ));
    }
    let expected = ExpectedConfig {
        command: receipt.command.clone(),
        args: receipt.args.clone(),
        tier: receipt.tier,
        fastctx_budget: receipt.fastctx_token_budget,
        tool_budgets: receipt.tool_budgets,
    };
    let patch_original = transaction::read_snapshot(&paths.dsh_patch)?;
    let patch_source = patch_original.as_deref().unwrap_or_default();
    if dsh_config::classify(patch_source, &expected)? != BlockState::Current {
        return Err(
            "DeepSeek Harness FastCtx block is missing or drifted; refusing to remove it. Restore the last applied block or clean it up manually."
                .to_string(),
        );
    }
    let patch_bytes =
        dsh_config::remove(patch_source, &expected, receipt.patch_inserted_separator)?;
    let patch_action = if patch_bytes.is_empty() && !receipt.patch.original_existed {
        FileAction::Delete
    } else {
        FileAction::Write(patch_bytes)
    };
    let agents_original = transaction::read_snapshot(&paths.dsh_agents)?;
    if agents::classify_managed_section(
        agents_original.as_deref().unwrap_or_default(),
        receipt.fastshell_enabled,
    ) != agents::ManagedSectionState::Current
    {
        return Err(
            "DeepSeek Harness AGENTS.md FastCtx guidance is missing or drifted; refusing to remove user changes."
                .to_string(),
        );
    }
    let agents_inserted_separator = agents_original
        .as_deref()
        .filter(|bytes| receipt.agents.applied_sha256 == sha256(bytes))
        .and(receipt.agents_inserted_separator);
    let agents_bytes = agents::remove_applied_section(
        agents_original.as_deref().unwrap_or_default(),
        agents_inserted_separator,
    )?;
    let agents_action = if agents_bytes.is_empty() && !receipt.agents.original_existed {
        FileAction::Delete
    } else {
        FileAction::Write(agents_bytes)
    };
    let mut next = saved;
    next.integrations.deepseek_harness = None;
    let no_other_host = next.integrations.codex.is_none();
    if no_other_host {
        next.installation = None;
    }
    let settings_action = if no_other_host {
        FileAction::Delete
    } else {
        FileAction::Write(settings::encode(&next)?)
    };
    let installed_original = transaction::read_snapshot(&paths.installed_binary)?;
    let binary_action = if no_other_host {
        FileAction::Delete
    } else {
        FileAction::Write(installed_original.clone().unwrap_or_default())
    };
    let changes = vec![
        FileChange {
            target: paths.dsh_patch.clone(),
            original: patch_original,
            action: patch_action,
            unix_mode: transaction::existing_unix_mode(&paths.dsh_patch).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: paths.dsh_agents.clone(),
            original: agents_original,
            action: agents_action,
            unix_mode: transaction::existing_unix_mode(&paths.dsh_agents).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: paths.fastctx_config.clone(),
            original: settings_original,
            action: settings_action,
            unix_mode: transaction::existing_unix_mode(&paths.fastctx_config).or(Some(0o600)),
            locked_binary_fallback: false,
        },
        FileChange {
            target: paths.installed_binary.clone(),
            original: installed_original,
            action: binary_action,
            unix_mode: transaction::existing_unix_mode(&paths.installed_binary).or(Some(0o755)),
            locked_binary_fallback: false,
        },
    ];
    let mut plan = Plan {
        changes,
        patch_state: BlockState::Current,
        dsh_home: paths.dsh_dir.clone(),
        binary_changed: no_other_host,
        complete_removal: no_other_host,
        complete_plan: None,
    };
    if no_other_host {
        plan.complete_plan = Some(Box::new(crate::control::apply::plan_complete_removal(
            paths,
            current_executable,
            plan.changes.clone(),
        )?));
    }
    Ok(plan)
}

pub fn commit_unapply(plan: Plan) -> Result<usize, String> {
    if plan.complete_removal {
        let complete = plan.complete_plan.ok_or_else(|| {
            "The complete DeepSeek Harness Unapply preview expired. Preview again.".to_string()
        })?;
        let changed = complete
            .preview()
            .iter()
            .filter(|item| item.action != crate::control::apply::PreviewAction::Unchanged)
            .count();
        crate::control::apply::commit_unapply(*complete)?;
        return Ok(changed);
    }
    let changed = plan
        .changes
        .iter()
        .filter(|change| change.is_changed())
        .count();
    transaction::commit(&plan.changes)?;
    Ok(changed)
}

pub fn status(paths: &ControlPaths) -> Result<(String, String), String> {
    let saved = settings::load(paths)?;
    let Some(receipt) = saved.integrations.deepseek_harness.as_ref() else {
        return Ok((
            "not connected".to_string(),
            format!(
                "DSH home: {} (source: {})",
                crate::paths::display_path(&paths.dsh_dir),
                paths.dsh_home_source.as_str()
            ),
        ));
    };
    let patch = transaction::read_snapshot(&paths.dsh_patch)?.unwrap_or_default();
    let expected = ExpectedConfig {
        command: receipt.command.clone(),
        args: receipt.args.clone(),
        tier: receipt.tier,
        fastctx_budget: receipt.fastctx_token_budget,
        tool_budgets: receipt.tool_budgets,
    };
    let state = dsh_config::classify(&patch, &expected)?;
    let agents = transaction::read_snapshot(&paths.dsh_agents)?.unwrap_or_default();
    let agents_state = agents::classify_managed_section(&agents, receipt.fastshell_enabled);
    let installation_issue = match saved.installation.as_ref() {
        None => Some("shared installation receipt is missing".to_string()),
        Some(installation)
            if installation.binary_path != receipt.command
                || receipt.command != crate::paths::display_path(&paths.installed_binary) =>
        {
            Some("shared installation path does not match the DSH command".to_string())
        }
        Some(installation) => match fs::read(&paths.installed_binary) {
            Ok(bytes) if sha256(&bytes) != installation.binary_sha256 => {
                Some("stable binary hash does not match the installation receipt".to_string())
            }
            Ok(_) => None,
            Err(error) => Some(format!("stable binary cannot be read: {error}")),
        },
    };
    let mcp_issue = if installation_issue.is_none() {
        crate::control::doctor::probe_mcp(
            &paths.installed_binary,
            crate::server::ServerOptions {
                enable_shell: receipt.fastshell_enabled,
            },
        )
        .err()
    } else {
        None
    };
    let (label, issue) = match state {
        BlockState::Missing => (
            "partial",
            Some("managed patch block is missing".to_string()),
        ),
        BlockState::Drifted => (
            "drifted",
            Some("managed patch block differs from the Apply receipt".to_string()),
        ),
        BlockState::Malformed(error) => ("conflicted", Some(error)),
        BlockState::Current => match agents_state {
            agents::ManagedSectionState::Current => match installation_issue.or(mcp_issue) {
                Some(error) => ("unhealthy", Some(error)),
                None => ("connected", None),
            },
            agents::ManagedSectionState::Missing => (
                "partial",
                Some("managed AGENTS.md guidance is missing".to_string()),
            ),
            agents::ManagedSectionState::Drifted | agents::ManagedSectionState::KnownLegacy => (
                "drifted",
                Some("managed AGENTS.md guidance differs from the Apply receipt".to_string()),
            ),
            agents::ManagedSectionState::Malformed(error) => ("conflicted", Some(error)),
        },
    };
    Ok((
        label.to_string(),
        format!(
            "DSH home: {} (source: {}), patch: {}, timeout: {}ms{}",
            crate::paths::display_path(&paths.dsh_dir),
            paths.dsh_home_source.as_str(),
            crate::paths::display_path(&paths.dsh_patch),
            dsh_config::TOOL_TIMEOUT_MS,
            issue.map_or_else(String::new, |issue| format!(", issue: {issue}"))
        ),
    ))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::apply::{
        ApplyOptions as CodexApplyOptions, UnapplyOptions, commit_apply as commit_codex_apply,
        commit_unapply as commit_codex_unapply, plan_apply as plan_codex_apply,
        plan_unapply as plan_codex_unapply,
    };

    fn fixture() -> (tempfile::TempDir, ControlPaths, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let paths = ControlPaths::for_home(temp.path());
        let executable = temp.path().join(if cfg!(windows) {
            "source.exe"
        } else {
            "source"
        });
        fs::write(&executable, b"binary").unwrap();
        (temp, paths, executable)
    }

    fn apply_codex(paths: &ControlPaths, executable: &Path) {
        let plan = plan_codex_apply(
            paths,
            CodexApplyOptions {
                tier: Tier::Standard,
                tool_budgets: ToolBudgetPreferences::default(),
                output_guard_enabled: true,
                fastshell_enabled: false,
                current_executable: executable.to_path_buf(),
            },
        )
        .unwrap();
        commit_codex_apply(plan, true).unwrap();
    }

    fn apply_dsh(paths: &ControlPaths, executable: &Path) {
        let plan = plan_apply(
            paths,
            ApplyOptions {
                tier: Tier::Standard,
                tool_budgets: ToolBudgetPreferences::default(),
                fastshell_enabled: false,
                current_executable: executable.to_path_buf(),
            },
        )
        .unwrap();
        commit_apply(plan).unwrap();
    }

    #[test]
    fn either_host_can_be_removed_first_without_breaking_the_other() {
        let (_temp, paths, executable) = fixture();
        apply_codex(&paths, &executable);
        apply_dsh(&paths, &executable);

        let dsh_unapply = plan_unapply(&paths, executable.clone()).unwrap();
        assert!(!dsh_unapply.complete_removal);
        commit_unapply(dsh_unapply).unwrap();
        let after_dsh = settings::load(&paths).unwrap();
        assert!(after_dsh.integrations.codex.is_some());
        assert!(after_dsh.integrations.deepseek_harness.is_none());
        assert!(after_dsh.installation.is_some());
        assert!(paths.installed_binary.exists());

        apply_dsh(&paths, &executable);
        let codex_unapply = plan_codex_unapply(
            &paths,
            UnapplyOptions {
                current_executable: executable.clone(),
            },
        )
        .unwrap();
        commit_codex_unapply(codex_unapply).unwrap();
        let after_codex = settings::load(&paths).unwrap();
        assert!(after_codex.integrations.codex.is_none());
        assert!(after_codex.integrations.deepseek_harness.is_some());
        assert!(after_codex.installation.is_some());
        assert!(paths.installed_binary.exists());
        assert!(paths.dsh_patch.exists());
    }

    #[test]
    fn removing_dsh_as_the_last_host_uses_complete_cleanup() {
        let (temp, paths, executable) = fixture();
        apply_dsh(&paths, &executable);
        let plan = plan_unapply(&paths, executable).unwrap();
        assert!(plan.complete_removal);
        commit_unapply(plan).unwrap();
        assert!(!paths.dsh_patch.exists());
        assert!(!paths.dsh_agents.exists());
        assert!(!paths.fastctx_dir.exists());
        assert!(temp.path().exists());
    }
}
