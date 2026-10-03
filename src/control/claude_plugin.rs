//! Plugin ownership and transactions through Claude Code's public CLI.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN: &str = "fastctx-world@fastctx";
pub(crate) const UNAVAILABLE: &str =
    "Claude Code was not found; World plugin operations were skipped.";

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PluginInstallation {
    id: String,
    scope: String,
    enabled: Option<bool>,
    #[serde(rename = "installPath")]
    install_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Marketplace {
    name: String,
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginReceipt {
    pub world_dir: PathBuf,
    pub marketplace_added: bool,
    pub plugin_added: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marketplace: Option<Marketplace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin: Option<PluginInstallation>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PluginState {
    marketplace: Option<Marketplace>,
    installations: Vec<PluginInstallation>,
}

impl PluginState {
    fn user_plugin(&self) -> Option<&PluginInstallation> {
        self.installations
            .iter()
            .find(|p| p.id == PLUGIN && p.scope == "user")
    }

    fn check_source(&self, world: &Path) -> Result<(), String> {
        if let Some(marketplace) = &self.marketplace
            && (marketplace.source.as_deref() != Some("directory")
                || marketplace.path.as_deref().is_none_or(|path| {
                    !paths_match(path, &crate::town::claude::marketplace_dir(world))
                }))
        {
            return Err(
                "Claude Code marketplace fastctx points at a different source.".to_string(),
            );
        }
        if let Some(plugin) = self.user_plugin()
            && plugin
                .install_path
                .as_deref()
                .is_none_or(|path| !paths_match(path, &crate::town::claude::plugin_dir(world)))
        {
            return Err(
                "Claude Code plugin fastctx-world@fastctx has a different user installation."
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn check_installed(&self, world: &Path) -> Result<(), String> {
        self.check_source(world)?;
        if self.marketplace.is_none() {
            return Err("Claude Code marketplace fastctx is missing.".to_string());
        }
        if self.user_plugin().is_none() {
            return Err("Claude Code World plugin is not installed at user scope.".to_string());
        }
        self.check_enabled()?;
        Ok(())
    }

    fn check_enabled(&self) -> Result<(), String> {
        if self
            .user_plugin()
            .is_some_and(|plugin| plugin.enabled != Some(true))
        {
            return Err("Claude Code plugin fastctx-world@fastctx is not enabled.".to_string());
        }
        Ok(())
    }

    fn check_marketplace_removal(&self) -> Result<(), String> {
        if self
            .installations
            .iter()
            .any(|p| p.id != PLUGIN || p.scope != "user")
        {
            return Err("Claude Code marketplace fastctx contains another installation; FastCtx will not remove it.".to_string());
        }
        Ok(())
    }
}

// Paths reported by the CLI are compared without opening the reported locations.
pub(crate) fn paths_match(actual: &Path, expected: &Path) -> bool {
    normalize_path(actual) == normalize_path(expected)
}

fn normalize_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(crate::paths::display_path(path).to_ascii_lowercase());
    path.components().collect()
}

fn run(home: &Path, args: &[String]) -> Result<Option<Vec<u8>>, String> {
    let output = match Command::new("claude")
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot run claude {}: {error}", args.join(" "))),
    };
    if !output.status.success() {
        return Err(format!(
            "claude {} exited {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(Some(output.stdout))
}

pub(crate) fn snapshot(home: &Path) -> Result<Option<PluginState>, String> {
    let args = ["plugin", "list", "--json"].map(String::from);
    let Some(plugins) = run(home, &args)? else {
        return Ok(None);
    };
    let args = ["plugin", "marketplace", "list", "--json"].map(String::from);
    let marketplaces = run(home, &args)?.ok_or(UNAVAILABLE)?;
    let plugins: Value = serde_json::from_slice(&plugins)
        .map_err(|error| format!("Cannot parse claude plugin list --json: {error}"))?;
    let marketplaces: Value = serde_json::from_slice(&marketplaces)
        .map_err(|error| format!("Cannot parse claude plugin marketplace list --json: {error}"))?;
    let mut installations: Vec<PluginInstallation> = plugins
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(PluginInstallation {
                id: value.get("id")?.as_str()?.to_string(),
                scope: value.get("scope")?.as_str()?.to_string(),
                enabled: value.get("enabled").and_then(Value::as_bool),
                install_path: value
                    .get("installPath")
                    .and_then(Value::as_str)
                    .map(|path| normalize_path(Path::new(path))),
            })
        })
        .filter(|plugin| plugin.id.ends_with("@fastctx"))
        .collect();
    installations.sort();
    let marketplace = marketplaces
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(Marketplace {
                name: value.get("name")?.as_str()?.to_string(),
                source: value
                    .get("source")
                    .and_then(Value::as_str)
                    .map(String::from),
                path: value
                    .get("path")
                    .and_then(Value::as_str)
                    .map(|path| normalize_path(Path::new(path))),
            })
        })
        .find(|marketplace| marketplace.name == "fastctx");
    Ok(Some(PluginState {
        marketplace,
        installations,
    }))
}

pub(crate) fn check_receipt(state: &PluginState, receipt: &PluginReceipt) -> Result<(), String> {
    state.check_source(&receipt.world_dir)?;
    state.check_enabled()?;
    if receipt
        .marketplace
        .as_ref()
        .is_some_and(|marketplace| Some(marketplace) != state.marketplace.as_ref())
        || receipt
            .plugin
            .as_ref()
            .is_some_and(|plugin| Some(plugin) != state.user_plugin())
    {
        return Err("Claude Code World plugin state differs from the Apply receipt.".to_string());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct PluginPlan {
    pub home: PathBuf,
    world_dir: PathBuf,
    original: Option<PluginState>,
    previous: Option<PluginReceipt>,
    pub install: bool,
}

fn install_commands(world: &Path) -> [Vec<String>; 2] {
    let [mut marketplace, plugin] = crate::town::claude::install_commands(world);
    marketplace.extend(["--scope".to_string(), "user".to_string()]);
    [marketplace, plugin]
}

impl PluginPlan {
    pub fn new(
        home: &Path,
        world: &Path,
        install: bool,
        previous: Option<&PluginReceipt>,
    ) -> Result<Self, String> {
        if previous.is_some_and(|previous| !paths_match(&previous.world_dir, world)) {
            return Err(
                "The Claude World receipt points at a different World directory.".to_string(),
            );
        }
        let original = snapshot(home)?;
        if let Some(state) = &original {
            if let Some(previous) = previous {
                check_receipt(state, previous)?;
                if !install && previous.marketplace_added {
                    state.check_marketplace_removal()?;
                }
            }
            if install {
                state.check_source(world)?;
                state.check_enabled()?;
                if state.marketplace.is_none() && !state.installations.is_empty() {
                    return Err(
                        "Claude Code marketplace fastctx is missing for an existing installation."
                            .to_string(),
                    );
                }
            }
        }
        let plan = Self {
            home: home.to_owned(),
            world_dir: world.to_owned(),
            original,
            previous: previous.cloned(),
            install,
        };
        if install && !plan.commands().is_empty() {
            for manifest in [
                crate::town::claude::marketplace_dir(world).join(".claude-plugin/marketplace.json"),
                crate::town::claude::plugin_dir(world).join(".claude-plugin/plugin.json"),
            ] {
                if !manifest.is_file() {
                    return Err(format!(
                        "World plugin file {} is missing; start the World node before Apply.",
                        crate::paths::display_path(&manifest)
                    ));
                }
            }
        }
        Ok(plan)
    }

    pub fn note(&self) -> Option<&'static str> {
        self.original.is_none().then_some(UNAVAILABLE)
    }

    pub fn commands(&self) -> Vec<Vec<String>> {
        let Some(state) = &self.original else {
            return Vec::new();
        };
        let mut commands = Vec::new();
        if self.install {
            let [marketplace, plugin] = install_commands(&self.world_dir);
            if state.marketplace.is_none() {
                commands.push(marketplace);
            }
            if state.user_plugin().is_none() {
                commands.push(plugin);
            }
        } else if let Some(previous) = &self.previous {
            let [plugin, marketplace] = crate::town::claude::uninstall_commands();
            if previous.marketplace_added && state.marketplace.is_some() {
                commands.push(marketplace);
            } else if previous.plugin_added && state.user_plugin().is_some() {
                commands.push(plugin);
            }
        }
        commands
    }

    pub fn validate(&self) -> Result<(), String> {
        if snapshot(&self.home)? != self.original {
            return Err("Claude Code World plugin state changed after the preview.".to_string());
        }
        Ok(())
    }

    pub fn run(&self, args: &[String]) -> Result<(), String> {
        run(&self.home, args)?.ok_or(UNAVAILABLE)?;
        Ok(())
    }

    pub fn finish(&self) -> Result<Option<PluginReceipt>, String> {
        let Some(before) = &self.original else {
            return Ok(self.previous.clone());
        };
        let after = snapshot(&self.home)?.ok_or(UNAVAILABLE)?;
        if !self.install {
            if let Some(previous) = &self.previous {
                if previous.marketplace_added && after.marketplace.is_some() {
                    return Err(
                        "Claude Code marketplace fastctx remains after removal.".to_string()
                    );
                }
                if previous.plugin_added && after.user_plugin().is_some() {
                    return Err(
                        "Claude Code World plugin remains installed after removal.".to_string()
                    );
                }
            }
            return Ok(None);
        }
        after.check_installed(&self.world_dir)?;
        Ok(Some(PluginReceipt {
            world_dir: self.world_dir.clone(),
            marketplace_added: before.marketplace.is_none()
                || self.previous.as_ref().is_some_and(|r| r.marketplace_added),
            plugin_added: before.user_plugin().is_none()
                || self.previous.as_ref().is_some_and(|r| r.plugin_added),
            marketplace: after.marketplace.clone(),
            plugin: after.user_plugin().cloned(),
        }))
    }

    pub fn rollback(&self) -> Result<(), String> {
        let Some(before) = &self.original else {
            return Ok(());
        };
        let current = snapshot(&self.home)?.ok_or(UNAVAILABLE)?;
        current.check_source(&self.world_dir)?;
        if self.install {
            let [plugin, marketplace] = crate::town::claude::uninstall_commands();
            if before.marketplace.is_none() && current.marketplace.is_some() {
                current.check_marketplace_removal()?;
                self.run(&marketplace)?;
            } else if before.user_plugin().is_none() && current.user_plugin().is_some() {
                self.run(&plugin)?;
            }
        } else {
            let [marketplace, plugin] = install_commands(&self.world_dir);
            if before.marketplace.is_some() && current.marketplace.is_none() {
                self.run(&marketplace)?;
            }
            if before.user_plugin().is_some() && current.user_plugin().is_none() {
                self.run(&plugin)?;
            }
        }
        if snapshot(&self.home)?.as_ref() != Some(before) {
            return Err(
                "Claude Code World plugin state differs from the state before the commands."
                    .to_string(),
            );
        }
        Ok(())
    }
}
