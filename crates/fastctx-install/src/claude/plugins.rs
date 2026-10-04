//! Public CLI contract, checked against Claude Code 2.1.287's bundled program
//! and https://code.claude.com/docs/en/plugins/cli-reference.
//!
//! `enabled` is the effective switch in the command's settings context, not
//! the switch of the installation's scope. Run every command in the same
//! caller-selected user context. No private Claude registry is accessed here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plan::ClaudePlan;

/// An argv vector for the public `claude` executable; never shell text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommand {
    pub args: Vec<String>,
}

impl PluginCommand {
    fn new(args: &[&str]) -> Self {
        Self { args: args.iter().map(|s| (*s).to_owned()).collect() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub id: String,
    pub version: String,
    pub scope: String,
    pub enabled: bool,
    pub install_path: PathBuf,
    pub installed_at: Option<String>,
    pub last_updated: Option<String>,
    pub project_path: Option<PathBuf>,
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marketplace {
    pub name: String,
    pub source: String,
    pub path: Option<PathBuf>,
    pub install_location: Option<PathBuf>,
}

/// Both public listings, taken in the same execution context. Malformed
/// output is an error, including a JSON object where an array was promised.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginState {
    pub installations: Vec<Installation>,
    pub marketplaces: Vec<Marketplace>,
}

impl PluginState {
    pub fn parse(plugins: &[u8], marketplaces: &[u8]) -> Result<Self, String> {
        let installations: Vec<Installation> =
            serde_json::from_slice(plugins).map_err(|e| format!("Invalid `claude plugin list --json` output: {e}"))?;
        let marketplaces: Vec<Marketplace> = serde_json::from_slice(marketplaces)
            .map_err(|e| format!("Invalid `claude plugin marketplace list --json` output: {e}"))?;
        for (i, market) in marketplaces.iter().enumerate() {
            if marketplaces[..i].iter().any(|m| m.name == market.name) {
                return Err(format!("Claude listed marketplace {} more than once", market.name));
            }
        }
        for (i, plugin) in installations.iter().enumerate() {
            if installations[..i]
                .iter()
                .any(|p| p.id == plugin.id && p.scope == plugin.scope && p.project_path == plugin.project_path)
            {
                return Err(format!(
                    "Claude listed installation {} more than once in scope {}",
                    plugin.id, plugin.scope
                ));
            }
        }
        Ok(Self { installations, marketplaces })
    }
}

/// Execution and durability belong to the integrating application. Persist
/// every checkpoint before returning success, and serialize installations
/// against each other. `run` must return an error for every nonzero exit and
/// include the command, exit status, stdout and stderr in that error.
pub trait PluginHost {
    fn run(&mut self, command: &PluginCommand) -> Result<Vec<u8>, String>;
    fn checkpoint(&mut self, receipt: &PluginReceipt) -> Result<(), String>;
}

pub fn snapshot(host: &mut impl PluginHost) -> Result<PluginState, String> {
    let plugins = host.run(&PluginCommand::new(&["plugin", "list", "--json"]))?;
    let marketplaces = host.run(&PluginCommand::new(&["plugin", "marketplace", "list", "--json"]))?;
    PluginState::parse(&plugins, &marketplaces)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Target {
    marketplace: PathBuf,
    name: String,
    plugin: String,
}

impl Target {
    fn new(plan: &ClaudePlan) -> Result<Self, String> {
        if !plan.marketplace.is_absolute() || plan.marketplace.to_str().is_none() {
            return Err("Claude marketplace must have an absolute UTF-8 path".into());
        }
        Ok(Self {
            marketplace: plan.marketplace.clone(),
            name: plan.marketplace_name.into(),
            plugin: plan.plugin.into(),
        })
    }
    fn id(&self) -> String {
        format!("{}@{}", self.plugin, self.name)
    }
    fn market<'a>(&self, state: &'a PluginState) -> Option<&'a Marketplace> {
        state.marketplaces.iter().find(|m| m.name == self.name)
    }
    fn installation<'a>(&self, state: &'a PluginState) -> Option<&'a Installation> {
        state.installations.iter().find(|p| p.id == self.id() && p.scope == "user")
    }
    fn members<'a>(&'a self, state: &'a PluginState) -> impl Iterator<Item = &'a Installation> {
        state.installations.iter().filter(|p| p.id.ends_with(&format!("@{}", self.name)))
    }
    fn check_market(&self, market: &Marketplace) -> Result<(), String> {
        if market.source != "directory"
            || market.path.as_ref().is_none_or(|p| !paths_match(p, &self.marketplace))
            || market.install_location.as_ref().is_none_or(|p| !paths_match(p, &self.marketplace))
        {
            return Err(format!("Claude marketplace {} points at a different source: {:?}", self.name, market));
        }
        Ok(())
    }
    fn check_plugin(&self, plugin: &Installation) -> Result<(), String> {
        // 2.1.287 records the cached copy here even when sessions load the
        // local source in place. The marketplace, not this path, is the source.
        if !plugin.install_path.is_absolute() {
            return Err(format!("Claude plugin {} has no absolute installation path", plugin.id));
        }
        if !plugin.enabled {
            return Err(format!("Claude plugin {} is disabled in this settings context", plugin.id));
        }
        if !plugin.errors.is_empty() {
            return Err(format!("Claude plugin {} has load errors: {}", plugin.id, plugin.errors.join("; ")));
        }
        Ok(())
    }
    fn preflight(&self, state: &PluginState) -> Result<(), String> {
        if let Some(market) = self.market(state) {
            self.check_market(market)?;
        } else if self.members(state).next().is_some() {
            return Err(format!("Claude marketplace {} is missing for existing installations", self.name));
        }
        for p in &state.installations {
            if p.id.split('@').next() == Some(&self.plugin) && (p.id != self.id() || p.scope != "user") {
                return Err(format!("Claude already has {} in {} scope; it was left unchanged", p.id, p.scope));
            }
        }
        if let Some(plugin) = self.installation(state) {
            self.check_plugin(plugin)?;
        }
        Ok(())
    }
    fn command(&self, action: Mutation) -> PluginCommand {
        match action {
            Mutation::AddMarketplace => PluginCommand::new(&[
                "plugin",
                "marketplace",
                "add",
                self.marketplace.to_str().expect("validated UTF-8"),
                "--scope",
                "user",
            ]),
            Mutation::InstallPlugin => PluginCommand::new(&["plugin", "install", &self.id(), "--scope", "user"]),
            Mutation::UninstallPlugin => {
                PluginCommand::new(&["plugin", "uninstall", &self.id(), "--scope", "user", "--keep-data"])
            }
            Mutation::RemoveMarketplace => {
                PluginCommand::new(&["plugin", "marketplace", "remove", &self.name, "--scope", "user"])
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mutation {
    AddMarketplace,
    InstallPlugin,
    UninstallPlugin,
    RemoveMarketplace,
}

/// Durable evidence of ownership and of a command which may have changed
/// Claude before it failed or the process stopped. Borrowed items have no
/// ownership flag. Do not discard this record while an error leaves work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginReceipt {
    target: Target,
    pub marketplace_added: bool,
    pub plugin_added: bool,
    marketplace: Option<Marketplace>,
    plugin: Option<Installation>,
    /// An install can write enabledPlugins before creating a listable record.
    /// This evidence survives subsequent failed uninstall attempts.
    pub unconfirmed_install: bool,
    pub pending: Option<Mutation>,
    /// A post-command public snapshot was obtained in the executing process.
    /// An intent alone cannot explain later changes made by the user.
    pub pending_observed: bool,
}

impl PluginReceipt {
    fn empty(target: Target) -> Self {
        Self {
            target,
            marketplace_added: false,
            plugin_added: false,
            marketplace: None,
            plugin: None,
            unconfirmed_install: false,
            pending: None,
            pending_observed: false,
        }
    }
    pub fn has_work(&self) -> bool {
        self.marketplace_added || self.plugin_added || self.pending.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct PluginFailure {
    pub message: String,
    /// Includes unfinished recovery. Always retain it if `has_work()`.
    pub receipt: Box<PluginReceipt>,
    pub rollback_error: Option<String>,
}

impl std::fmt::Display for PluginFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(error) = &self.rollback_error {
            write!(f, "; Claude plugin rollback stopped: {error}")?;
        }
        Ok(())
    }
}
impl std::error::Error for PluginFailure {}

/// The reviewable commands and baseline for one apply. `execute` rechecks the
/// baseline before changing anything, checkpoints each mutation, verifies its
/// result, and on failure removes only additions of this invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginApply {
    before: PluginState,
    original_receipt: PluginReceipt,
    actions: Vec<Mutation>,
}

impl PluginApply {
    pub fn prepare(plan: &ClaudePlan, before: PluginState, previous: Option<&PluginReceipt>) -> Result<Self, String> {
        let target = Target::new(plan)?;
        target.preflight(&before)?;
        let receipt = previous.cloned().unwrap_or_else(|| PluginReceipt::empty(target.clone()));
        if receipt.target != target {
            return Err("Claude receipt belongs to a different marketplace".into());
        }
        if receipt.pending.is_some() {
            return Err("Claude has an unfinished plugin command; recover its receipt before applying again".into());
        }
        check_owned(&receipt, &before, false)?;
        let mut actions = Vec::new();
        if target.market(&before).is_none() {
            actions.push(Mutation::AddMarketplace);
        }
        if target.installation(&before).is_none() {
            actions.push(Mutation::InstallPlugin);
        }
        Ok(Self { before, original_receipt: receipt, actions })
    }

    pub fn commands(&self) -> Vec<PluginCommand> {
        self.actions.iter().map(|&a| self.original_receipt.target.command(a)).collect()
    }

    pub fn execute(&self, host: &mut impl PluginHost) -> Result<PluginReceipt, PluginFailure> {
        let mut receipt = self.original_receipt.clone();
        let result = (|| {
            let current = snapshot(host)?;
            if relevant(&current, &receipt.target) != relevant(&self.before, &receipt.target) {
                return Err("Claude plugin state changed after preparation".into());
            }
            for &action in &self.actions {
                let before_command = receipt.clone();
                receipt.pending = Some(action);
                receipt.pending_observed = false;
                // Flags precede the command: even a failing command can change
                // settings or registry state before returning a nonzero exit.
                match action {
                    Mutation::AddMarketplace => receipt.marketplace_added = true,
                    Mutation::InstallPlugin => {
                        receipt.plugin_added = true;
                        receipt.unconfirmed_install = true;
                    }
                    _ => unreachable!(),
                }
                if let Err(error) = host.checkpoint(&receipt) {
                    receipt = before_command;
                    return Err(error);
                }
                let command_result = host.run(&receipt.target.command(action));
                let state = snapshot(host)?;
                receipt.pending_observed = true;
                capture(&mut receipt, &state);
                command_result?;
                match action {
                    Mutation::AddMarketplace => receipt
                        .target
                        .check_market(receipt.target.market(&state).ok_or("Claude did not add the marketplace")?)?,
                    Mutation::InstallPlugin => receipt.target.check_plugin(
                        receipt.target.installation(&state).ok_or("Claude did not install the plugin")?,
                    )?,
                    _ => unreachable!(),
                }
                // Adding a marketplace can trigger dependency installation in
                // Claude. An installation that appeared before our install
                // command is not ours to reinstall or claim.
                if action == Mutation::AddMarketplace {
                    receipt.target.preflight(&state)?;
                    if receipt.target.installation(&state).is_some() {
                        return Err("Claude plugin appeared during marketplace registration; it was not claimed".into());
                    }
                }
                receipt.pending = None;
                receipt.pending_observed = false;
                host.checkpoint(&receipt)?;
            }
            Ok(())
        })();
        if let Err(message) = result {
            let mut rollback_error = cleanup(host, &mut receipt, true, Some(&self.original_receipt)).err();
            if let Err(error) = host.checkpoint(&receipt) {
                rollback_error = Some(match rollback_error {
                    Some(old) => format!("{old}; saving remaining receipt: {error}"),
                    None => error,
                });
            }
            return Err(PluginFailure { message, receipt: Box::new(receipt), rollback_error });
        }
        Ok(receipt)
    }

    /// Also use this after a later settings/file publication failure. Settings
    /// must be reread after this call before undoing their single managed key.
    pub fn rollback(
        &self,
        host: &mut impl PluginHost,
        applied: &PluginReceipt,
    ) -> Result<PluginReceipt, PluginFailure> {
        let mut additions = applied.clone();
        if additions.pending.is_some()
            || (additions.plugin_added && additions.plugin.is_none())
            || (additions.marketplace_added && additions.marketplace.is_none())
        {
            return Err(PluginFailure {
                message: "Rollback requires a completed Apply receipt; use unapply to recover an interrupted command"
                    .into(),
                receipt: Box::new(additions),
                rollback_error: None,
            });
        }
        if additions.target != self.original_receipt.target {
            return Err(PluginFailure {
                message: "Cannot roll back a receipt for another marketplace".into(),
                receipt: Box::new(additions),
                rollback_error: None,
            });
        }
        if let Err(message) = cleanup(host, &mut additions, true, Some(&self.original_receipt)) {
            return Err(PluginFailure { message, receipt: Box::new(additions), rollback_error: None });
        }
        host.checkpoint(&self.original_receipt).map_err(|message| PluginFailure {
            message,
            receipt: Box::new(self.original_receipt.clone()),
            rollback_error: None,
        })?;
        Ok(self.original_receipt.clone())
    }
}

/// Withdraws only owned installations. A failure returns the remaining
/// receipt and exact stopping point; it never reinstalls a removed plugin.
/// Retry this function with that receipt to finish an interrupted withdrawal.
pub fn unapply(host: &mut impl PluginHost, previous: &PluginReceipt) -> Result<PluginReceipt, PluginFailure> {
    let mut receipt = previous.clone();
    if let Err(message) = cleanup(host, &mut receipt, false, None) {
        let checkpoint_error = host.checkpoint(&receipt).err();
        return Err(PluginFailure { message, receipt: Box::new(receipt), rollback_error: checkpoint_error });
    }
    Ok(receipt)
}

fn capture(receipt: &mut PluginReceipt, state: &PluginState) {
    if receipt.marketplace_added && receipt.marketplace.is_none() {
        receipt.marketplace = receipt.target.market(state).cloned();
    }
    if receipt.plugin_added && receipt.plugin.is_none() {
        receipt.plugin = receipt.target.installation(state).cloned();
        if receipt.plugin.is_some() {
            receipt.unconfirmed_install = false;
        }
    }
}

fn check_owned(receipt: &PluginReceipt, state: &PluginState, removing: bool) -> Result<(), String> {
    if (receipt.marketplace_added || receipt.plugin_added)
        && let Some(market) = receipt.target.market(state)
    {
        receipt.target.check_market(market)?;
    }
    if receipt.marketplace_added
        && let Some(market) = receipt.target.market(state)
    {
        receipt.target.check_market(market)?;
        if receipt.marketplace.as_ref().is_some_and(|old| old != market) {
            return Err("Claude marketplace changed since Apply; left unchanged".into());
        }
    } else if receipt.marketplace_added && !removing {
        return Err("Owned Claude marketplace is missing".into());
    }
    if receipt.plugin_added
        && let Some(plugin) = receipt.target.installation(state)
    {
        if let Some(old) = &receipt.plugin {
            let mut expected = old.clone();
            if receipt.pending_observed
                && matches!(receipt.pending, Some(Mutation::UninstallPlugin | Mutation::RemoveMarketplace))
            {
                expected.enabled = plugin.enabled;
                expected.errors = plugin.errors.clone();
            }
            if &expected != plugin {
                return Err("Claude plugin changed since Apply; left unchanged".into());
            }
        }
        if !removing {
            receipt.target.check_plugin(plugin)?;
        }
    } else if receipt.plugin_added && !removing {
        return Err("Owned Claude plugin is missing".into());
    }
    Ok(())
}

fn cleanup(
    host: &mut impl PluginHost,
    receipt: &mut PluginReceipt,
    rollback: bool,
    keep: Option<&PluginReceipt>,
) -> Result<(), String> {
    let remove_plugin = (receipt.plugin_added || receipt.pending == Some(Mutation::InstallPlugin))
        && !keep.is_some_and(|r| r.plugin_added);
    let remove_marketplace = (receipt.marketplace_added || receipt.pending == Some(Mutation::AddMarketplace))
        && !keep.is_some_and(|r| r.marketplace_added);
    if !remove_plugin && !remove_marketplace {
        return Ok(());
    }
    let mut state = snapshot(host)?;
    // An intent identifies possibly affected items; the checks below still
    // require observed identities before removing something that now exists.
    if receipt.pending == Some(Mutation::AddMarketplace) {
        receipt.marketplace_added = true;
    }
    if receipt.pending == Some(Mutation::InstallPlugin) {
        receipt.plugin_added = true;
    }
    if rollback {
        // This invocation knows it actually called the command. Recovery from
        // disk has only an intent, which cannot establish who created an item.
        capture(receipt, &state);
    } else if (remove_plugin && receipt.plugin.is_none() && receipt.target.installation(&state).is_some())
        || (remove_marketplace && receipt.marketplace.is_none() && receipt.target.market(&state).is_some())
    {
        return Err("An interrupted Claude command has no recorded result; ownership of the current installation is unknown, so it was left unchanged".into());
    }
    check_owned(receipt, &state, true)?;
    // An install may fail after writing enabledPlugins, before it has any
    // installation record. Uninstall cannot address that state. Removing our
    // own otherwise empty marketplace can; a borrowed marketplace cannot be
    // sacrificed to hide the incomplete rollback.
    let partial_install = remove_plugin
        && remove_marketplace
        && receipt.target.installation(&state).is_none()
        && receipt.unconfirmed_install
        && receipt.target.members(&state).next().is_none();
    if remove_plugin && !partial_install {
        if let Some(plugin) = receipt.target.installation(&state)
            && !rollback
            && !plugin.enabled
            && receipt.pending != Some(Mutation::UninstallPlugin)
            && receipt.pending != Some(Mutation::InstallPlugin)
        {
            return Err("Owned Claude plugin was disabled after Apply; left unchanged".into());
        }
        let must_run = receipt.target.installation(&state).is_some() || receipt.unconfirmed_install;
        if must_run {
            receipt.pending = Some(Mutation::UninstallPlugin);
            receipt.pending_observed = false;
            host.checkpoint(receipt)?;
            let result = host.run(&receipt.target.command(Mutation::UninstallPlugin));
            state = snapshot(host)?;
            receipt.pending_observed = true;
            result?;
            if receipt.target.installation(&state).is_some() {
                return Err("Claude plugin remains after uninstall".into());
            }
        }
        receipt.plugin_added = false;
        receipt.plugin = None;
        receipt.unconfirmed_install = false;
        receipt.pending = None;
        receipt.pending_observed = false;
        host.checkpoint(receipt)?;
    }
    if remove_marketplace {
        check_owned(receipt, &state, true)?;
        if let Some(market) = receipt.target.market(&state) {
            receipt.target.check_market(market)?;
        }
        if receipt.target.members(&state).next().is_some() {
            return Err("Owned Claude marketplace now has another installation; marketplace left unchanged".into());
        }
        if receipt.target.market(&state).is_some() {
            receipt.pending = Some(Mutation::RemoveMarketplace);
            receipt.pending_observed = false;
            host.checkpoint(receipt)?;
            let result = host.run(&receipt.target.command(Mutation::RemoveMarketplace));
            state = snapshot(host)?;
            receipt.pending_observed = true;
            result?;
            if receipt.target.market(&state).is_some() {
                return Err("Claude marketplace remains after removal (another scope may still declare it)".into());
            }
        }
        receipt.marketplace_added = false;
        receipt.marketplace = None;
        if partial_install {
            receipt.plugin_added = false;
            receipt.plugin = None;
            receipt.unconfirmed_install = false;
        }
        receipt.pending = None;
        receipt.pending_observed = false;
        host.checkpoint(receipt)?;
    }
    Ok(())
}

fn relevant<'a>(state: &'a PluginState, target: &Target) -> (Vec<&'a Installation>, Option<&'a Marketplace>) {
    (
        state
            .installations
            .iter()
            .filter(|p| p.id.ends_with(&format!("@{}", target.name)) || p.id.split('@').next() == Some(&target.plugin))
            .collect(),
        target.market(state),
    )
}

fn paths_match(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| {
        let path: PathBuf = p.components().collect();
        if cfg!(windows) {
            path.to_string_lossy().replace('\\', "/").trim_start_matches("//?/").to_ascii_lowercase()
        } else {
            path.to_string_lossy().into_owned()
        }
    };
    normalize(a) == normalize(b)
}
