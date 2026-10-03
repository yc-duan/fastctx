//! World-specific Claude host settings and the plugin command transaction.

use super::apply::{PreviewAction, PreviewDetail, PreviewItem, PreviewTarget};
use super::claude_plugin::{self, PluginPlan, PluginReceipt};
use super::json_setting::{self, JsonSettingReceipt};
use super::paths::ControlPaths;
use super::settings::{self, FastCtxSettings};
use super::targets::{AgentTarget, json_config};
use super::transaction::{self, FileAction, FileChange};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const LIMIT_PATH: &[&str] = &["env", "MAX_MCP_OUTPUT_TOKENS"];
const LIMIT: &str = "\"200000\"";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClaudeWorldReceipt {
    pub settings_path: PathBuf,
    pub settings_existed: bool,
    pub output_limit: JsonSettingReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginReceipt>,
}

#[derive(Clone, Debug)]
pub(crate) struct ClaudeWorldPlan {
    plugin: PluginPlan,
    receipt: ClaudeWorldReceipt,
    previous_limit: Option<JsonSettingReceipt>,
    original: Option<Vec<u8>>,
    projected: Option<Vec<u8>>,
    limit_detail: PreviewDetail,
}

pub(crate) fn plan(
    control: &ControlPaths,
    enrolled: bool,
    previous: Option<&ClaudeWorldReceipt>,
) -> Result<Option<ClaudeWorldPlan>, String> {
    if !enrolled && previous.is_none() {
        return Ok(None);
    }
    let path = control.home.join(".claude/settings.json");
    if previous.is_some_and(|previous| !claude_plugin::paths_match(&previous.settings_path, &path))
    {
        return Err("The Claude World receipt points at a different settings file.".to_string());
    }
    let plugin = PluginPlan::new(
        &control.home,
        &control.fastctx_dir.join("world"),
        enrolled,
        previous.and_then(|r| r.plugin.as_ref()),
    )?;
    let original = transaction::read_snapshot(&path)?;
    let previous_limit = previous.map(|r| r.output_limit.clone());
    let (projected, receipt, limit_detail) = if enrolled {
        let (bytes, output_limit) = json_setting::set(
            original.as_deref().unwrap_or_default(),
            LIMIT_PATH,
            LIMIT,
            previous_limit.as_ref(),
        )?;
        (
            Some(bytes),
            ClaudeWorldReceipt {
                settings_path: path,
                settings_existed: previous
                    .map(|r| r.settings_existed)
                    .unwrap_or(original.is_some()),
                output_limit,
                plugin: previous.and_then(|r| r.plugin.clone()),
            },
            PreviewDetail::kept("env.MAX_MCP_OUTPUT_TOKENS = \"200000\""),
        )
    } else {
        let receipt = previous
            .expect("removing a World connection requires its receipt")
            .clone();
        let (bytes, detail) = restore_limit(original.clone(), &receipt)?;
        (bytes, receipt, detail)
    };
    Ok(Some(ClaudeWorldPlan {
        plugin,
        receipt,
        previous_limit,
        original,
        projected,
        limit_detail,
    }))
}

fn restore_limit(
    bytes: Option<Vec<u8>>,
    receipt: &ClaudeWorldReceipt,
) -> Result<(Option<Vec<u8>>, PreviewDetail), String> {
    let source = bytes.as_deref().unwrap_or_default();
    // The host limit is shared: a later user value survives removal, like Codex's limit.
    if json_config::inspect(source, LIMIT_PATH)?.as_deref()
        != Some(&receipt.output_limit.applied_hash)
    {
        return Ok((
            bytes,
            PreviewDetail::kept("env.MAX_MCP_OUTPUT_TOKENS was changed after Apply"),
        ));
    }
    let restored = json_setting::restore(source, LIMIT_PATH, &receipt.output_limit)?;
    let bytes = if restored.is_empty() && !receipt.settings_existed {
        None
    } else {
        Some(restored)
    };
    let detail = match &receipt.output_limit.before {
        Some(raw) => PreviewDetail::kept(format!("env.MAX_MCP_OUTPUT_TOKENS = {raw}")),
        None => PreviewDetail::removed("env.MAX_MCP_OUTPUT_TOKENS = \"200000\""),
    };
    Ok((bytes, detail))
}

impl ClaudeWorldPlan {
    pub fn paths(&self) -> &[PathBuf] {
        std::slice::from_ref(&self.receipt.settings_path)
    }

    pub fn receipt(&self) -> Option<ClaudeWorldReceipt> {
        self.plugin.install.then(|| self.receipt.clone())
    }

    pub fn note(&self) -> Option<&'static str> {
        self.plugin.note()
    }

    pub fn preview(&self) -> Vec<PreviewItem> {
        let mut details = vec![self.limit_detail.clone()];
        for args in self.plugin.commands() {
            let command = format!(
                "claude {}",
                args.iter()
                    .map(|arg| {
                        if arg.chars().any(char::is_whitespace) {
                            format!("\"{}\"", arg.replace('"', "\\\""))
                        } else {
                            arg.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            details.push(if self.plugin.install {
                PreviewDetail::kept(command)
            } else {
                PreviewDetail::removed(command)
            });
        }
        if let Some(note) = self.note() {
            details.push(PreviewDetail::kept(note));
        }
        vec![PreviewItem {
            path: self.receipt.settings_path.clone(),
            action: if self.is_empty() {
                PreviewAction::Unchanged
            } else if self.projected.is_none() && self.plugin.commands().is_empty() {
                PreviewAction::Delete
            } else {
                PreviewAction::Modify
            },
            target: PreviewTarget::AgentConfig,
            details,
        }]
    }

    pub fn is_empty(&self) -> bool {
        self.projected == self.original && self.plugin.commands().is_empty()
    }

    fn change(&self, original: Option<Vec<u8>>, desired: Option<Vec<u8>>) -> FileChange {
        FileChange {
            target: self.receipt.settings_path.clone(),
            original,
            action: desired.map(FileAction::Write).unwrap_or(FileAction::Delete),
            unix_mode: transaction::existing_unix_mode(&self.receipt.settings_path).or(Some(0o600)),
            locked_binary_fallback: false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        transaction::validate_external(&[
            self.change(self.original.clone(), self.original.clone())
        ])?;
        self.plugin.validate()
    }

    pub fn commit(&self, changes: &[FileChange]) -> Result<usize, String> {
        transaction::validate(changes)?;
        self.validate()?;
        let mut commands_started = false;
        let result = (|| {
            for args in self.plugin.commands() {
                commands_started = true;
                self.plugin.run(&args)?;
            }
            let plugin_receipt = self.plugin.finish()?;
            let after_commands = transaction::read_snapshot(&self.receipt.settings_path)?;
            if json_config::inspect(after_commands.as_deref().unwrap_or_default(), LIMIT_PATH)?
                != json_config::inspect(self.original.as_deref().unwrap_or_default(), LIMIT_PATH)?
            {
                return Err(
                    "Claude Code env.MAX_MCP_OUTPUT_TOKENS changed during the plugin commands."
                        .to_string(),
                );
            }
            // CLI changes remain intact; only the documented output limit is edited here.
            let (desired, receipt) = if self.plugin.install {
                let (bytes, output_limit) = json_setting::set(
                    after_commands.as_deref().unwrap_or_default(),
                    LIMIT_PATH,
                    LIMIT,
                    self.previous_limit.as_ref(),
                )?;
                let mut receipt = self.receipt.clone();
                receipt.output_limit = output_limit;
                receipt.plugin = plugin_receipt;
                (Some(bytes), Some(receipt))
            } else {
                (
                    restore_limit(after_commands.clone(), &self.receipt)?.0,
                    None,
                )
            };
            let mut pending = changes.to_vec();
            if self.plugin.install {
                for change in &mut pending {
                    if change.target == self.plugin.home.join(".fastctx/config.toml")
                        && let FileAction::Write(bytes) = &mut change.action
                    {
                        let mut settings: FastCtxSettings = toml_edit::de::from_str(
                            std::str::from_utf8(bytes).map_err(|error| error.to_string())?,
                        )
                        .map_err(|error| {
                            format!("Cannot parse the pending Apply receipt: {error}")
                        })?;
                        let target = settings
                            .applied_targets
                            .get_mut(AgentTarget::ClaudeCode.id())
                            .ok_or("The pending Claude Code Apply receipt is missing.")?;
                        target.claude_world = receipt.clone();
                        *bytes = settings::encode(&settings)?;
                    }
                }
            }
            let change = self.change(after_commands, desired);
            let extra_count = usize::from(change.is_changed()) + self.plugin.commands().len();
            pending.push(change);
            transaction::commit(&pending)?;
            Ok(extra_count)
        })();
        if let Err(error) = result {
            if !commands_started {
                return Err(error);
            }
            return match self.plugin.rollback() {
                Ok(()) => Err(format!("{error} Claude plugin changes were rolled back.")),
                Err(rollback) => Err(format!(
                    "{error} Claude plugin rollback also failed: {rollback}"
                )),
            };
        }
        result
    }
}

pub(crate) fn inspect(
    control: &ControlPaths,
    receipt: Option<&ClaudeWorldReceipt>,
) -> Result<Option<&'static str>, String> {
    let enrolled = crate::world::is_enrolled(control);
    let Some(receipt) = receipt else {
        return if enrolled {
            Err("Claude Code has no World host settings receipt; run fastctx apply.".to_string())
        } else {
            Ok(None)
        };
    };
    if !claude_plugin::paths_match(
        &receipt.settings_path,
        &control.home.join(".claude/settings.json"),
    ) {
        return Err("The Claude World receipt points at a different settings file.".to_string());
    }
    if !enrolled {
        return Err("Claude Code still has World host settings after this machine left its World; run fastctx apply.".to_string());
    }
    let bytes = transaction::read_snapshot(&receipt.settings_path)?;
    if json_config::raw_value(bytes.as_deref().unwrap_or_default(), LIMIT_PATH)?
        .map(|raw| serde_json::from_str::<serde_json::Value>(&raw))
        .transpose()
        .map_err(|error| error.to_string())?
        != Some(serde_json::Value::String("200000".to_string()))
    {
        return Err(
            "Claude Code env.MAX_MCP_OUTPUT_TOKENS differs from the World value 200000."
                .to_string(),
        );
    }
    let Some(state) = claude_plugin::snapshot(&control.home)? else {
        return Ok(Some(claude_plugin::UNAVAILABLE));
    };
    state.check_installed(&control.fastctx_dir.join("world"))?;
    let plugin = receipt
        .plugin
        .as_ref()
        .ok_or("Claude Code has no World plugin receipt; run fastctx apply.")?;
    claude_plugin::check_receipt(&state, plugin)?;
    Ok(None)
}
