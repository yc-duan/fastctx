//! Claude Code owns plugin installation; FastCtx owns only the changes its commands made.

use super::json_setting::{self, JsonSettingReceipt};
use super::targets::json_config;
use super::transaction::{self, FileAction, FileChange};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) type PluginFiles = [Option<Vec<u8>>; 3];

const PLUGIN: &str = "fastctx-world@fastctx";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginValue {
    MarketplaceDeclaration,
    Enabled,
    Marketplace,
    RegistryVersion,
    Installation,
}

impl PluginValue {
    fn file(self) -> usize {
        match self {
            Self::MarketplaceDeclaration | Self::Enabled => 0,
            Self::Marketplace => 1,
            Self::RegistryVersion | Self::Installation => 2,
        }
    }

    fn path(self) -> &'static [&'static str] {
        match self {
            Self::MarketplaceDeclaration => &["extraKnownMarketplaces", "fastctx"],
            Self::Enabled => &["enabledPlugins", PLUGIN],
            Self::Marketplace => &["fastctx"],
            Self::RegistryVersion => &["version"],
            Self::Installation => &["plugins", PLUGIN],
        }
    }
}

const VALUES: [PluginValue; 5] = [
    PluginValue::MarketplaceDeclaration,
    PluginValue::Enabled,
    PluginValue::Marketplace,
    PluginValue::RegistryVersion,
    PluginValue::Installation,
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginEntryReceipt {
    pub key: PluginValue,
    pub edit: JsonSettingReceipt,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginReceipt {
    pub world_dir: PathBuf,
    /// An existing marketplace remains borrowed even when Apply adds its user declaration.
    pub marketplace_added: bool,
    /// Enabling an existing user installation does not give Apply permission to uninstall it.
    pub plugin_added: bool,
    pub entries: Vec<PluginEntryReceipt>,
    pub files_existed: [bool; 3],
}

#[derive(Clone, Debug)]
pub(crate) struct PluginPlan {
    pub home: PathBuf,
    pub paths: [PathBuf; 3],
    pub originals: PluginFiles,
    pub world_dir: PathBuf,
    pub previous: Option<PluginReceipt>,
    pub install: bool,
    pub add_marketplace: bool,
    pub add_plugin: bool,
    created_directories: Vec<PathBuf>,
}

pub(crate) fn snapshot(home: &Path) -> Result<([PathBuf; 3], PluginFiles), String> {
    let config = home.join(".claude");
    let paths = [
        config.join("settings.json"),
        config.join("plugins/known_marketplaces.json"),
        config.join("plugins/installed_plugins.json"),
    ];
    let originals = [
        transaction::read_snapshot(&paths[0])?,
        transaction::read_snapshot(&paths[1])?,
        transaction::read_snapshot(&paths[2])?,
    ];
    Ok((paths, originals))
}

fn value(files: &PluginFiles, key: PluginValue) -> Result<Option<Value>, String> {
    json_config::raw_value(files[key.file()].as_deref().unwrap_or_default(), key.path())?
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|error| format!("Cannot parse Claude plugin metadata: {error}"))
        })
        .transpose()
}

fn same_path(value: Option<&str>, path: &Path) -> bool {
    value.is_some_and(|value| paths_match(Path::new(value), path))
}

pub(crate) fn paths_match(actual: &Path, expected: &Path) -> bool {
    actual == expected
        || matches!((dunce::canonicalize(actual), dunce::canonicalize(expected)), (Ok(a), Ok(b)) if a == b)
}

fn source_matches(value: &Value, world: &Path) -> bool {
    value.pointer("/source/source").and_then(Value::as_str) == Some("directory")
        && same_path(
            value.pointer("/source/path").and_then(Value::as_str),
            &crate::town::claude::marketplace_dir(world),
        )
}

pub(crate) fn installed(files: &PluginFiles, world: &Path) -> Result<(bool, bool), String> {
    let declaration = value(files, PluginValue::MarketplaceDeclaration)?;
    let marketplace = value(files, PluginValue::Marketplace)?;
    for entry in [&declaration, &marketplace].into_iter().flatten() {
        if !source_matches(entry, world) {
            return Err(
                "Claude Code marketplace fastctx points at a different source.".to_string(),
            );
        }
    }
    if let Some(entry) = &marketplace
        && !same_path(
            entry.get("installLocation").and_then(Value::as_str),
            &crate::town::claude::marketplace_dir(world),
        )
    {
        return Err("Claude Code marketplace fastctx has a different installLocation.".to_string());
    }
    let enabled = value(files, PluginValue::Enabled)?;
    if enabled
        .as_ref()
        .is_some_and(|value| value != &Value::Bool(true))
    {
        return Err("Claude Code plugin fastctx-world@fastctx is disabled or has a different enabled value.".to_string());
    }
    let entry = value(files, PluginValue::Installation)?;
    let mut user_installed = false;
    if let Some(entry) = entry {
        if value(files, PluginValue::RegistryVersion)? != Some(Value::from(2)) {
            return Err("Claude Code plugin registry is not version 2.".to_string());
        }
        let entries = entry
            .as_array()
            .ok_or("Claude Code plugin registry is not version 2.")?;
        for entry in entries {
            if entry.get("scope").and_then(Value::as_str) == Some("user") {
                if user_installed
                    || !same_path(
                        entry.get("installPath").and_then(Value::as_str),
                        &crate::town::claude::plugin_dir(world),
                    )
                {
                    return Err("Claude Code plugin fastctx-world@fastctx has a different user installation.".to_string());
                }
                user_installed = true;
            }
        }
    }
    Ok((
        declaration.is_some() && marketplace.is_some(),
        user_installed && enabled.is_some(),
    ))
}

pub(crate) fn check_receipt(files: &PluginFiles, receipt: &PluginReceipt) -> Result<(), String> {
    for entry in &receipt.entries {
        if json_config::inspect(
            files[entry.key.file()].as_deref().unwrap_or_default(),
            entry.key.path(),
        )?
        .as_deref()
            != Some(&entry.edit.applied_hash)
        {
            return Err(format!(
                "Claude Code plugin entry {} drifted after Apply; FastCtx will not change it.",
                entry.key.path().join(".")
            ));
        }
    }
    if receipt.marketplace_added {
        let registry: Value = serde_json::from_slice(files[2].as_deref().unwrap_or(b"{}"))
            .map_err(|error| format!("Cannot parse Claude plugin registry: {error}"))?;
        if registry
            .get("plugins")
            .and_then(Value::as_object)
            .is_some_and(|plugins| {
                plugins
                    .keys()
                    .any(|key| key != PLUGIN && key.ends_with("@fastctx"))
            })
        {
            return Err("Claude Code marketplace fastctx contains another installed plugin; FastCtx will not remove it.".to_string());
        }
    }
    Ok(())
}

impl PluginPlan {
    pub fn new(
        home: &Path,
        world: &Path,
        install: bool,
        previous: Option<&PluginReceipt>,
    ) -> Result<Self, String> {
        let (paths, originals) = snapshot(home)?;
        if let Some(previous) = previous {
            if !paths_match(&previous.world_dir, world) {
                return Err(
                    "The Claude World receipt points at a different World directory.".to_string(),
                );
            }
            check_receipt(&originals, previous)?;
        }
        let (marketplace, plugin) = if install {
            installed(&originals, world)?
        } else {
            (false, false)
        };
        let add_marketplace = install && !marketplace;
        let add_plugin = install && !plugin;
        let mut created_directories = Vec::new();
        for directory in [home.join(".claude/plugins"), home.join(".claude")] {
            match std::fs::metadata(&directory) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    return Err(format!(
                        "{} is not a directory.",
                        crate::paths::display_path(&directory)
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    created_directories.push(directory)
                }
                Err(error) => {
                    return Err(format!(
                        "Cannot inspect {}: {error}",
                        crate::paths::display_path(&directory)
                    ));
                }
            }
        }
        if add_marketplace || add_plugin {
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
        Ok(Self {
            home: home.to_owned(),
            paths,
            originals,
            world_dir: world.to_owned(),
            previous: previous.cloned(),
            install,
            add_marketplace,
            add_plugin,
            created_directories,
        })
    }

    pub fn commands(&self) -> Vec<Vec<String>> {
        if self.install {
            let [marketplace, plugin] = crate::town::claude::install_commands(&self.world_dir);
            let mut commands = Vec::new();
            if self.add_marketplace {
                commands.push(marketplace);
            }
            if self.add_plugin {
                commands.push(plugin);
            }
            commands
        } else {
            let mut commands = Vec::new();
            if let Some(previous) = &self.previous {
                let [plugin, marketplace] = crate::town::claude::uninstall_commands();
                if previous.plugin_added {
                    commands.push(plugin);
                }
                if previous.marketplace_added {
                    commands.push(marketplace);
                }
            }
            commands
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        transaction::validate_external(&self.changes(&self.originals, &self.originals))
    }

    pub fn run(&self, args: &[String]) -> Result<(), String> {
        let result = Command::new("claude")
            .args(args)
            .current_dir(&self.home)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .output()
            .map_err(|error| format!("Cannot run claude {}: {error}", args.join(" ")))?;
        if !result.status.success() {
            return Err(format!(
                "claude {} exited {}: {}",
                args.join(" "),
                result.status,
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        Ok(())
    }

    pub fn project_install(
        &self,
        after: &PluginFiles,
        mut files: PluginFiles,
    ) -> Result<(PluginFiles, PluginReceipt), String> {
        if installed(after, &self.world_dir)? != (true, true) {
            return Err("Claude Code did not register the World plugin at user scope.".to_string());
        }
        let marketplace_added = self.add_marketplace && self.marketplace_is_new()?;
        let plugin_added = self.add_plugin && self.plugin_is_new()?;
        let mut receipt = self.previous.clone().unwrap_or_else(|| PluginReceipt {
            world_dir: self.world_dir.clone(),
            marketplace_added,
            plugin_added,
            entries: Vec::new(),
            files_existed: self.originals.each_ref().map(Option::is_some),
        });
        receipt.marketplace_added |= marketplace_added;
        receipt.plugin_added |= plugin_added;
        for key in VALUES {
            let before_hash = json_config::inspect(
                self.originals[key.file()].as_deref().unwrap_or_default(),
                key.path(),
            )?;
            let after_hash =
                json_config::inspect(after[key.file()].as_deref().unwrap_or_default(), key.path())?;
            if before_hash == after_hash {
                continue;
            }
            let raw = json_config::raw_value(
                after[key.file()].as_deref().unwrap_or_default(),
                key.path(),
            )?
            .ok_or_else(|| {
                format!(
                    "Claude Code removed plugin entry {} during installation.",
                    key.path().join(".")
                )
            })?;
            let previous = receipt
                .entries
                .iter()
                .find(|entry| entry.key == key)
                .map(|entry| &entry.edit);
            let (bytes, edit) = json_setting::set(
                files[key.file()].as_deref().unwrap_or_default(),
                key.path(),
                &raw,
                previous,
            )?;
            files[key.file()] = Some(bytes);
            receipt.entries.retain(|entry| entry.key != key);
            receipt.entries.push(PluginEntryReceipt { key, edit });
        }
        Ok((files, receipt))
    }

    pub fn project_remove(&self, mut files: PluginFiles) -> Result<PluginFiles, String> {
        if let Some(receipt) = &self.previous {
            for entry in receipt.entries.iter().rev() {
                if entry.key == PluginValue::RegistryVersion && entry.edit.before.is_none() {
                    let registry: Value = serde_json::from_slice(
                        files[2].as_deref().unwrap_or(b"{}"),
                    )
                    .map_err(|error| format!("Cannot parse Claude plugin registry: {error}"))?;
                    if registry
                        .get("plugins")
                        .and_then(Value::as_object)
                        .is_some_and(|plugins| !plugins.is_empty())
                    {
                        continue;
                    }
                }
                files[entry.key.file()] = Some(json_setting::restore(
                    files[entry.key.file()].as_deref().unwrap_or_default(),
                    entry.key.path(),
                    &entry.edit,
                )?);
            }
            for (index, existed) in receipt.files_existed.iter().enumerate() {
                if !existed
                    && files[index]
                        .as_deref()
                        .is_some_and(|bytes| bytes.is_empty())
                {
                    files[index] = None;
                }
            }
        }
        Ok(files)
    }

    pub fn changes(&self, originals: &PluginFiles, desired: &PluginFiles) -> Vec<FileChange> {
        self.paths
            .iter()
            .enumerate()
            .map(|(index, path)| FileChange {
                target: path.clone(),
                original: originals[index].clone(),
                action: desired[index]
                    .clone()
                    .map(FileAction::Write)
                    .unwrap_or(FileAction::Delete),
                unix_mode: transaction::existing_unix_mode(path).or(Some(0o600)),
                locked_binary_fallback: false,
            })
            .collect()
    }

    pub fn rollback(&self, expected: &PluginFiles, commands_started: bool) -> Result<(), String> {
        let (_, current) = snapshot(&self.home)?;
        if &current != expected {
            return Err("Claude plugin files changed during the transaction; FastCtx will not overwrite them.".to_string());
        }
        self.check_unrelated(&current)?;
        if commands_started {
            if self.install {
                let [uninstall, remove] = crate::town::claude::uninstall_commands();
                if self.add_plugin
                    && self.plugin_is_new()?
                    && value(&current, PluginValue::Installation)?
                        != value(&self.originals, PluginValue::Installation)?
                {
                    self.run(&uninstall)?;
                }
                if self.add_marketplace
                    && self.marketplace_is_new()?
                    && value(&current, PluginValue::MarketplaceDeclaration)?.is_some()
                    && (value(&current, PluginValue::Marketplace)?
                        != value(&self.originals, PluginValue::Marketplace)?
                        || value(&current, PluginValue::MarketplaceDeclaration)?
                            != value(&self.originals, PluginValue::MarketplaceDeclaration)?)
                {
                    self.run(&remove)?;
                }
            } else if let Some(previous) = &self.previous {
                let [marketplace, plugin] = crate::town::claude::install_commands(&self.world_dir);
                if previous.marketplace_added {
                    self.run(&marketplace)?;
                }
                if previous.plugin_added {
                    self.run(&plugin)?;
                }
            }
        }
        let (_, current) = snapshot(&self.home)?;
        self.check_unrelated(&current)?;
        transaction::commit(&self.changes(&current, &self.originals))?;
        for directory in &self.created_directories {
            match std::fs::remove_dir(directory) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => {
                    return Err(format!(
                        "Cannot remove {}: {error}",
                        crate::paths::display_path(directory)
                    ));
                }
            }
        }
        Ok(())
    }

    fn marketplace_is_new(&self) -> Result<bool, String> {
        for key in [
            PluginValue::Marketplace,
            PluginValue::MarketplaceDeclaration,
            PluginValue::Installation,
        ] {
            if value(&self.originals, key)?.is_some() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn plugin_is_new(&self) -> Result<bool, String> {
        Ok(!value(&self.originals, PluginValue::Installation)?
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("scope").and_then(Value::as_str) == Some("user"))
            }))
    }

    pub fn check_removed(&self, files: &PluginFiles) -> Result<(), String> {
        if let Some(receipt) = &self.previous {
            if receipt.plugin_added
                && value(files, PluginValue::Installation)?
                    .as_ref()
                    .and_then(Value::as_array)
                    .is_some_and(|entries| {
                        entries
                            .iter()
                            .any(|entry| entry.get("scope").and_then(Value::as_str) == Some("user"))
                    })
            {
                return Err(
                    "Claude Code did not uninstall the World plugin at user scope.".to_string(),
                );
            }
            if receipt.marketplace_added
                && value(files, PluginValue::MarketplaceDeclaration)?.is_some()
            {
                return Err(
                    "Claude Code did not remove the World marketplace at user scope.".to_string(),
                );
            }
        }
        Ok(())
    }

    pub fn check_unrelated(&self, current: &PluginFiles) -> Result<(), String> {
        for (index, current) in current.iter().enumerate() {
            if unowned(&self.originals[index], index)? != unowned(current, index)? {
                return Err(format!(
                    "{} changed outside the World plugin entries during installation; FastCtx will not overwrite it.",
                    crate::paths::display_path(&self.paths[index])
                ));
            }
        }
        Ok(())
    }
}

fn unowned(bytes: &Option<Vec<u8>>, file: usize) -> Result<Value, String> {
    let source = bytes.as_deref().unwrap_or_default();
    let mut value: Value = if source.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(source)
            .map_err(|error| format!("Cannot parse Claude plugin metadata: {error}"))?
    };
    for key in VALUES.into_iter().filter(|key| key.file() == file) {
        remove_value(&mut value, key.path());
    }
    Ok(value)
}

fn remove_value(value: &mut Value, path: &[&str]) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if path.len() == 1 {
        object.remove(path[0]);
    } else if let Some(child) = object.get_mut(path[0]) {
        remove_value(child, &path[1..]);
        if child.as_object().is_some_and(|object| object.is_empty()) {
            object.remove(path[0]);
        }
    }
}
