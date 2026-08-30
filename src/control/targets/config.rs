//! Target-specific MCP entry shapes over shared source-preserving editors.

use super::{AgentTarget, ConfigKind};
use crate::control::settings::{JsoncConfigReceipt, ToolBudgets};
use crate::server_manifest::EnabledTools;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct ConfigEdit {
    pub bytes: Vec<u8>,
    pub managed_hash: String,
    pub jsonc_receipt: Option<JsoncConfigReceipt>,
}

pub(crate) struct ConfigApplyRequest<'a> {
    pub target: AgentTarget,
    pub original: &'a [u8],
    pub executable: &'a Path,
    pub tools: EnabledTools,
    pub global_budget: usize,
    pub tool_budgets: ToolBudgets,
    pub owned_hash: Option<&'a str>,
    pub previous_jsonc: Option<&'a JsoncConfigReceipt>,
}

pub(crate) fn apply_config(request: ConfigApplyRequest<'_>) -> Result<ConfigEdit, String> {
    let ConfigApplyRequest {
        target,
        original,
        executable,
        tools,
        global_budget,
        tool_budgets,
        owned_hash,
        previous_jsonc,
    } = request;
    if target.config_kind() == ConfigKind::CodexToml {
        return Err("Codex configuration is handled by the Codex TOML adapter.".to_string());
    }
    let desired = desired_entry(target, executable, tools, global_budget, tool_budgets);
    match target.config_kind() {
        ConfigKind::CommonJsonc | ConfigKind::NestedJsonc | ConfigKind::OpenCodeJsonc => {
            let edit = super::json_config::apply(
                original,
                property_path(target),
                &desired,
                owned_hash,
                previous_jsonc,
            )?;
            Ok(ConfigEdit {
                bytes: edit.bytes,
                managed_hash: edit.managed_hash,
                jsonc_receipt: Some(edit.receipt),
            })
        }
        ConfigKind::TraeYaml => {
            let edit = super::trae_config::apply(original, &desired, owned_hash)?;
            Ok(ConfigEdit {
                bytes: edit.bytes,
                managed_hash: edit.managed_hash,
                jsonc_receipt: None,
            })
        }
        ConfigKind::CodexToml => unreachable!(),
    }
}

pub(crate) fn disconnect_config(
    target: AgentTarget,
    original: &[u8],
    owned_hash: &str,
    jsonc_receipt: Option<&JsoncConfigReceipt>,
) -> Result<Vec<u8>, String> {
    match target.config_kind() {
        ConfigKind::CommonJsonc | ConfigKind::NestedJsonc | ConfigKind::OpenCodeJsonc => {
            super::json_config::disconnect(
                original,
                property_path(target),
                owned_hash,
                jsonc_receipt,
            )
        }
        ConfigKind::TraeYaml => super::trae_config::disconnect(original, owned_hash),
        ConfigKind::CodexToml => {
            Err("Codex configuration is handled by the Codex TOML adapter.".to_string())
        }
    }
}

pub(crate) fn inspect_config(
    target: AgentTarget,
    original: &[u8],
) -> Result<Option<String>, String> {
    match target.config_kind() {
        ConfigKind::CommonJsonc | ConfigKind::NestedJsonc | ConfigKind::OpenCodeJsonc => {
            super::json_config::inspect(original, property_path(target))
        }
        ConfigKind::TraeYaml => super::trae_config::inspect(original),
        ConfigKind::CodexToml => {
            Err("Codex configuration is inspected by the Codex TOML adapter.".to_string())
        }
    }
}

fn property_path(target: AgentTarget) -> &'static [&'static str] {
    match target {
        AgentTarget::ClaudeCode
        | AgentTarget::Cursor
        | AgentTarget::VscodeCopilot
        | AgentTarget::Antigravity
        | AgentTarget::Qoder => &["mcpServers", "fastctx"],
        AgentTarget::Opencode => &["mcp", "fastctx"],
        AgentTarget::Zcode => &["mcp", "servers", "fastctx"],
        AgentTarget::Codex | AgentTarget::Trae => &[],
    }
}

fn desired_entry(
    target: AgentTarget,
    executable: &Path,
    tools: EnabledTools,
    global_budget: usize,
    tool_budgets: ToolBudgets,
) -> Value {
    let executable = crate::paths::display_path(executable);
    let args = vec![
        "serve".to_string(),
        "--tools".to_string(),
        tools.names().join(","),
    ];
    let environment = budget_environment(tools, global_budget, tool_budgets);
    match target {
        AgentTarget::Opencode => json!({
            "type": "local",
            "command": std::iter::once(executable).chain(args).collect::<Vec<_>>(),
            "environment": environment,
        }),
        AgentTarget::Trae => json!({
            "name": "fastctx",
            "type": "stdio",
            "command": executable,
            "args": args,
            "env": environment,
        }),
        AgentTarget::Codex => unreachable!(),
        _ => json!({
            "command": executable,
            "args": args,
            "env": environment,
        }),
    }
}

fn budget_environment(
    tools: EnabledTools,
    global: usize,
    budgets: ToolBudgets,
) -> serde_json::Map<String, Value> {
    let mut environment = serde_json::Map::new();
    environment.insert(
        "FASTCTX_TOKEN_BUDGET".to_string(),
        Value::String(global.to_string()),
    );
    for (tool, name, value) in [
        (
            "inspect_local_file",
            "FASTCTX_READ_TOKEN_BUDGET",
            budgets.read.resolve(global),
        ),
        (
            "grep",
            "FASTCTX_GREP_TOKEN_BUDGET",
            budgets.grep.resolve(global),
        ),
        (
            "glob",
            "FASTCTX_GLOB_TOKEN_BUDGET",
            budgets.glob.resolve(global),
        ),
        (
            "run",
            "FASTCTX_RUN_TOKEN_BUDGET",
            budgets.run.resolve(global),
        ),
        (
            "job_output",
            "FASTCTX_JOB_OUTPUT_TOKEN_BUDGET",
            budgets.job_output.resolve(global),
        ),
    ] {
        if tools.contains(tool)
            && let Some(value) = value
        {
            environment.insert(name.to_string(), Value::String(value.to_string()));
        }
    }
    environment
}
