//! Per-target connection state and ownership diagnostics.

use crate::control::agents;
use crate::control::codex_config::{self, ExpectedConfig};
use crate::control::paths::ControlPaths;
use crate::control::settings::{FastCtxSettings, TargetReceipt};
use crate::control::targets::{AgentTarget, guidance_managed_hash, inspect_config};
use crate::server_manifest::EnabledTools;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetConnectionState {
    NotConnected,
    Connected,
    NeedsAttention,
    PermissionDenied,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetStatus {
    pub target: AgentTarget,
    pub state: TargetConnectionState,
    pub enabled_tools: EnabledTools,
    pub effective_budget: usize,
    pub config_path: std::path::PathBuf,
    pub guidance_path: std::path::PathBuf,
    pub facts: Vec<String>,
}

pub fn inspect_target(
    control: &ControlPaths,
    settings: &FastCtxSettings,
    target: AgentTarget,
) -> TargetStatus {
    let paths = match target.paths(control) {
        Ok(paths) => paths,
        Err(error) => {
            return TargetStatus {
                target,
                state: TargetConnectionState::Error,
                enabled_tools: settings.selected_tools(target),
                effective_budget: target.budget_policy().clamp(settings.tier.fastctx_budget()),
                config_path: std::path::PathBuf::new(),
                guidance_path: std::path::PathBuf::new(),
                facts: vec![error],
            };
        }
    };
    let Some(receipt) = settings.target_receipt(target) else {
        return TargetStatus {
            target,
            state: TargetConnectionState::NotConnected,
            enabled_tools: settings.selected_tools(target),
            effective_budget: target.budget_policy().clamp(settings.tier.fastctx_budget()),
            config_path: paths.config,
            guidance_path: paths.guidance,
            facts: vec!["No Apply receipt for this target.".to_string()],
        };
    };
    let mut facts = vec![target.budget_policy().doctor_fact().to_string()];
    let state = match inspect_connected(
        control,
        target,
        receipt,
        settings.installation.as_ref(),
        &paths,
        &mut facts,
    ) {
        Ok(()) => TargetConnectionState::Connected,
        Err(TargetIssue::Permission(message)) => {
            facts.push(message);
            TargetConnectionState::PermissionDenied
        }
        Err(TargetIssue::Drift(message)) => {
            facts.push(message);
            TargetConnectionState::NeedsAttention
        }
        Err(TargetIssue::Invalid(message)) => {
            facts.push(message);
            TargetConnectionState::Error
        }
    };
    TargetStatus {
        target,
        state,
        enabled_tools: receipt.enabled_tools,
        effective_budget: receipt.fastctx_token_budget,
        config_path: paths.config,
        guidance_path: paths.guidance,
        facts,
    }
}

fn inspect_connected(
    control: &ControlPaths,
    target: AgentTarget,
    receipt: &TargetReceipt,
    installation_receipt: Option<&crate::control::settings::InstallationRecord>,
    paths: &crate::control::targets::TargetPaths,
    facts: &mut Vec<String>,
) -> Result<(), TargetIssue> {
    if !same_path(Path::new(&receipt.config.path), &paths.config)
        || !same_path(Path::new(&receipt.guidance.path), &paths.guidance)
    {
        return Err(TargetIssue::Drift(
            "Receipt paths no longer match this target's resolved user profile.".to_string(),
        ));
    }
    let config = read_required(&paths.config, "configuration")?;
    let guidance = read_required(&paths.guidance, "guidance")?;
    if target == AgentTarget::Codex {
        inspect_codex(receipt, installation_receipt, &config, &guidance)?;
    } else {
        let current = inspect_config(target, &config).map_err(TargetIssue::Invalid)?;
        if current.as_deref() != Some(receipt.config_entry_sha256.as_str()) {
            return Err(TargetIssue::Drift(
                "Managed MCP entry is missing or differs from the Apply receipt.".to_string(),
            ));
        }
        let current_guidance =
            guidance_managed_hash(target, &guidance).map_err(TargetIssue::Invalid)?;
        if current_guidance.as_deref() != Some(receipt.guidance_managed_sha256.as_str()) {
            return Err(TargetIssue::Drift(
                "Managed guidance is missing or differs from the Apply receipt.".to_string(),
            ));
        }
    }
    let installation = fs::read(&control.installed_binary)
        .map_err(|error| classify_io(&control.installed_binary, "installed binary", error))?;
    let actual_binary = sha256(&installation);
    let installation_receipt = installation_receipt
        .ok_or_else(|| TargetIssue::Drift("Shared installation receipt is missing.".to_string()))?;
    if installation_receipt.binary_sha256 != actual_binary
        || !same_path(
            Path::new(&installation_receipt.command),
            &control.installed_binary,
        )
    {
        return Err(TargetIssue::Drift(
            "Installed binary differs from the shared installation receipt.".to_string(),
        ));
    }
    facts.push(format!(
        "{} enabled tools",
        receipt.enabled_tools.names().len()
    ));
    Ok(())
}

fn inspect_codex(
    receipt: &TargetReceipt,
    installation: Option<&crate::control::settings::InstallationRecord>,
    config: &[u8],
    guidance: &[u8],
) -> Result<(), TargetIssue> {
    let codex = receipt.codex.as_ref().ok_or_else(|| {
        TargetIssue::Invalid("Codex target receipt is missing Codex ownership facts.".to_string())
    })?;
    let expected = ExpectedConfig {
        command: installation
            .ok_or_else(|| {
                TargetIssue::Drift("Shared installation receipt is missing.".to_string())
            })?
            .command
            .clone(),
        tier: codex.tier,
        host_limit: codex.tool_output_token_limit,
        fastctx_budget: receipt.fastctx_token_budget,
        tool_budgets: codex.tool_budgets,
        enabled_tools: receipt.enabled_tools,
    };
    let drift = codex_config::drift_applied(
        config,
        &expected,
        codex.tool_output_token_limit,
        receipt.fastctx_token_budget,
        codex.tool_timeout_sec,
    )
    .map_err(TargetIssue::Invalid)?;
    if !drift.is_empty() {
        return Err(TargetIssue::Drift(format!(
            "Codex managed values drifted: {}.",
            drift.join(", ")
        )));
    }
    if agents::classify_managed_section_for_tools(guidance, receipt.enabled_tools)
        != agents::ManagedSectionState::Current
    {
        return Err(TargetIssue::Drift(
            "Codex managed guidance differs from the enabled-set contract.".to_string(),
        ));
    }
    Ok(())
}

enum TargetIssue {
    Permission(String),
    Drift(String),
    Invalid(String),
}

fn read_required(path: &Path, label: &str) -> Result<Vec<u8>, TargetIssue> {
    fs::read(path).map_err(|error| classify_io(path, label, error))
}

fn classify_io(path: &Path, label: &str, error: std::io::Error) -> TargetIssue {
    let message = format!(
        "Cannot read target {label} {}: {error}",
        crate::paths::display_path(path)
    );
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        TargetIssue::Permission(message)
    } else if error.kind() == std::io::ErrorKind::NotFound {
        TargetIssue::Drift(message)
    } else {
        TargetIssue::Invalid(message)
    }
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

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
