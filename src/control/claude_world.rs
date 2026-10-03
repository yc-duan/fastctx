//! World-specific Claude host settings and the plugin command transaction.

use super::apply::{PreviewAction, PreviewDetail, PreviewItem, PreviewTarget};
use super::claude_plugin::{self, PluginFiles, PluginPlan, PluginReceipt};
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
    projected_files: PluginFiles,
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
    let world = control.fastctx_dir.join("world");
    let plugin = PluginPlan::new(
        &control.home,
        &world,
        enrolled,
        previous.and_then(|r| r.plugin.as_ref()),
    )?;
    if let Some(previous) = previous
        && !claude_plugin::paths_match(&previous.settings_path, &plugin.paths[0])
    {
        return Err("The Claude World receipt points at a different settings file.".to_string());
    }
    let (projected_files, receipt, limit_detail) = if enrolled {
        let (bytes, output_limit) = json_setting::set(
            plugin.originals[0].as_deref().unwrap_or_default(),
            LIMIT_PATH,
            LIMIT,
            previous.map(|r| &r.output_limit),
        )?;
        let mut files = plugin.originals.clone();
        files[0] = Some(bytes);
        (
            files,
            ClaudeWorldReceipt {
                settings_path: plugin.paths[0].clone(),
                settings_existed: previous
                    .map(|r| r.settings_existed)
                    .unwrap_or(plugin.originals[0].is_some()),
                output_limit,
                plugin: previous.and_then(|r| r.plugin.clone()),
            },
            PreviewDetail::kept("env.MAX_MCP_OUTPUT_TOKENS = \"200000\""),
        )
    } else {
        let receipt = previous
            .expect("removing a World connection requires its receipt")
            .clone();
        let mut files = plugin.project_remove(plugin.originals.clone())?;
        let source = files[0].as_deref().unwrap_or_default();
        // The host limit is shared: a later user value survives removal, like Codex's limit.
        let limit_detail = if json_config::inspect(source, LIMIT_PATH)?.as_deref()
            == Some(&receipt.output_limit.applied_hash)
        {
            let bytes = json_setting::restore(source, LIMIT_PATH, &receipt.output_limit)?;
            files[0] = if bytes.is_empty() && !receipt.settings_existed {
                None
            } else {
                Some(bytes)
            };
            match &receipt.output_limit.before {
                Some(raw) => PreviewDetail::kept(format!("env.MAX_MCP_OUTPUT_TOKENS = {raw}")),
                None => PreviewDetail::removed("env.MAX_MCP_OUTPUT_TOKENS = \"200000\""),
            }
        } else {
            PreviewDetail::kept("env.MAX_MCP_OUTPUT_TOKENS was changed after Apply")
        };
        (files, receipt, limit_detail)
    };
    Ok(Some(ClaudeWorldPlan {
        plugin,
        receipt,
        projected_files,
        limit_detail,
    }))
}

impl ClaudeWorldPlan {
    pub fn paths(&self) -> &[PathBuf; 3] {
        &self.plugin.paths
    }

    pub fn receipt(&self) -> Option<ClaudeWorldReceipt> {
        self.plugin.install.then(|| self.receipt.clone())
    }

    pub fn preview(&self) -> Vec<PreviewItem> {
        let commands = self.plugin.commands();
        self.plugin
            .paths
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let mut details = if index == 0 {
                    vec![self.limit_detail.clone()]
                } else {
                    Vec::new()
                };
                details.extend(
                    commands
                        .iter()
                        .filter(|args| index == if args[1] == "marketplace" { 1 } else { 2 })
                        .map(|args| {
                            let command = format!("claude {}", args.join(" "));
                            if self.plugin.install {
                                PreviewDetail::kept(command)
                            } else {
                                PreviewDetail::removed(command)
                            }
                        }),
                );
                let command_changes_file = if index == 0 {
                    !commands.is_empty()
                } else {
                    !details.is_empty()
                };
                let changed = command_changes_file
                    || self.projected_files[index] != self.plugin.originals[index];
                if details.is_empty() && changed {
                    details.push(PreviewDetail::removed("World plugin ownership"));
                }
                PreviewItem {
                    path: path.clone(),
                    action: if !changed {
                        PreviewAction::Unchanged
                    } else if self.projected_files[index].is_none() && !self.plugin.install {
                        PreviewAction::Delete
                    } else {
                        PreviewAction::Modify
                    },
                    target: PreviewTarget::AgentConfig,
                    details,
                }
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.projected_files == self.plugin.originals && self.plugin.commands().is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        self.plugin.validate()
    }

    pub fn commit(&self, changes: &[FileChange]) -> Result<usize, String> {
        transaction::validate(changes)?;
        self.validate()?;
        let mut commands_started = false;
        let mut rollback_snapshot = self.plugin.originals.clone();
        let result = (|| {
            for args in self.plugin.commands() {
                commands_started = true;
                let result = self.plugin.run(&args);
                rollback_snapshot = claude_plugin::snapshot(&self.plugin.home)?.1;
                result?;
            }
            let (_, after_commands) = claude_plugin::snapshot(&self.plugin.home)?;
            if !commands_started && after_commands != self.plugin.originals {
                return Err(
                    "Claude plugin files changed after the preview; preview again and retry."
                        .to_string(),
                );
            }
            self.plugin.check_unrelated(&after_commands)?;
            let mut files = self.projected_files.clone();
            let receipt = if self.plugin.install {
                let (projected, plugin_receipt) =
                    self.plugin.project_install(&after_commands, files)?;
                files = projected;
                let mut receipt = self.receipt.clone();
                receipt.plugin = Some(plugin_receipt);
                Some(receipt)
            } else {
                self.plugin.check_removed(&after_commands)?;
                None
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
            let extra_count = self
                .plugin
                .changes(&self.plugin.originals, &files)
                .iter()
                .filter(|change| change.is_changed())
                .count();
            pending.extend(self.plugin.changes(&after_commands, &files));
            transaction::commit(&pending)?;
            Ok(extra_count)
        })();
        if let Err(error) = result {
            if !commands_started {
                return Err(error);
            }
            return match self.plugin.rollback(&rollback_snapshot, commands_started) {
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
) -> Result<(), String> {
    let enrolled = crate::world::is_enrolled(control);
    let Some(receipt) = receipt else {
        return if enrolled {
            Err("Claude Code has no World host settings receipt; run fastctx apply.".to_string())
        } else {
            Ok(())
        };
    };
    if !claude_plugin::paths_match(
        &receipt.settings_path,
        &control.home.join(".claude/settings.json"),
    ) {
        return Err("The Claude World receipt points at a different settings file.".to_string());
    }
    let (_, files) = claude_plugin::snapshot(&control.home)?;
    if json_config::raw_value(files[0].as_deref().unwrap_or_default(), LIMIT_PATH)?
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
    let plugin = receipt
        .plugin
        .as_ref()
        .ok_or("Claude Code has no World plugin receipt; run fastctx apply.")?;
    claude_plugin::check_receipt(&files, plugin)?;
    if claude_plugin::installed(&files, &control.fastctx_dir.join("world"))? != (true, true) {
        return Err("Claude Code World plugin is not installed at user scope.".to_string());
    }
    if !enrolled {
        return Err("Claude Code still has World host settings after this machine left its World; run fastctx apply.".to_string());
    }
    Ok(())
}
