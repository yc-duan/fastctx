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
        }
        self.plugins_preflight(state)
    }
    /// The checks of `preflight` but the marketplace's source, which an
    /// apply sets right.
    fn plugins_preflight(&self, state: &PluginState) -> Result<(), String> {
        for p in &state.installations {
            if p.id.split('@').next() == Some(&self.plugin) && (p.id != self.id() || p.scope != "user") {
                return Err(format!("Claude already has {} in {} scope; it was left unchanged", p.id, p.scope));
            }
        }
        if let Some(plugin) = self.installation(state)
            && !plugin.install_path.is_absolute()
        {
            return Err(format!("Claude plugin {} has no absolute installation path", plugin.id));
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
            Mutation::EnablePlugin => PluginCommand::new(&["plugin", "enable", &self.id(), "--scope", "user"]),
            Mutation::DisablePlugin => PluginCommand::new(&["plugin", "disable", &self.id(), "--scope", "user"]),
            Mutation::UpdatePlugin => PluginCommand::new(&["plugin", "update", &self.id(), "--scope", "user"]),
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
    EnablePlugin,
    DisablePlugin,
    /// Claude Code's record of the plugin brought to the marketplace's version.
    UpdatePlugin,
}

/// Progress for fastctx's reserved marketplace and user installation. Their
/// current state resolves interrupted commands; historical installer identity
/// is not an ownership requirement for these dedicated product names.
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
    /// This distinguishes a completed observation from an interrupted query.
    pub pending_observed: bool,
    /// The version Claude Code's record of the plugin was last brought to,
    /// so a record its update leaves as it was is not updated again.
    #[serde(default)]
    pub updated: Option<String>,
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
            updated: None,
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

/// Commands and baseline for one apply. Execution checkpoints mutations and
/// verifies their public results. Rollback restores the baseline's presence
/// and enabled state, rather than historical cache timestamps.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginApply {
    before: PluginState,
    original_receipt: PluginReceipt,
    actions: Vec<Mutation>,
    /// The version the plugin in the marketplace is written as.
    #[serde(default)]
    version: Option<String>,
}

impl PluginApply {
    pub fn prepare(plan: &ClaudePlan, before: PluginState, previous: Option<&PluginReceipt>) -> Result<Self, String> {
        let target = Target::new(plan)?;
        target.plugins_preflight(&before)?;
        if previous.is_some_and(|receipt| receipt.target != target) {
            return Err("Claude receipt belongs to a different marketplace".into());
        }
        let mut receipt = PluginReceipt::empty(target.clone());
        capture_current(&mut receipt, &before);
        receipt.updated = previous.and_then(|previous| previous.updated.clone());
        let mut actions = Vec::new();
        // The marketplace of the World's name is the World's: pointed at
        // another source, it is pointed at the World's again.
        let moved = target.market(&before).is_some_and(|market| target.check_market(market).is_err());
        if moved {
            actions.push(Mutation::RemoveMarketplace);
        }
        if moved || target.market(&before).is_none() {
            actions.push(Mutation::AddMarketplace);
        }
        match target.installation(&before) {
            None => actions.push(Mutation::InstallPlugin),
            Some(_) if moved => actions.push(Mutation::InstallPlugin),
            Some(plugin) if !plugin.errors.is_empty() => {
                actions.push(Mutation::UninstallPlugin);
                actions.push(Mutation::InstallPlugin);
            }
            Some(plugin) => {
                if !plugin.enabled {
                    actions.push(Mutation::EnablePlugin);
                }
                if stale(plugin, &receipt, plan.plugin_version) {
                    actions.push(Mutation::UpdatePlugin);
                }
            }
        }
        Ok(Self { before, original_receipt: receipt, actions, version: Some(plan.plugin_version.into()) })
    }

    pub fn commands(&self) -> Vec<PluginCommand> {
        self.actions.iter().map(|&action| self.original_receipt.target.command(action)).collect()
    }

    pub fn execute(&self, host: &mut impl PluginHost) -> Result<PluginReceipt, PluginFailure> {
        let mut receipt = self.original_receipt.clone();
        if let Err(message) = present(host, &mut receipt, self.version.as_deref()) {
            let rollback_error = baseline(host, &mut receipt, &self.before).err();
            return Err(PluginFailure { message, receipt: Box::new(receipt), rollback_error });
        }
        Ok(receipt)
    }

    /// The dedicated marketplace and plugin belong to fastctx. A lost command
    /// result is resolved from their current state, without guessing who ran it.
    pub fn rollback(
        &self,
        host: &mut impl PluginHost,
        applied: &PluginReceipt,
    ) -> Result<PluginReceipt, PluginFailure> {
        let mut receipt = applied.clone();
        if receipt.target != self.original_receipt.target {
            return Err(PluginFailure {
                message: "Cannot roll back a receipt for another marketplace".into(),
                receipt: Box::new(receipt),
                rollback_error: None,
            });
        }
        baseline(host, &mut receipt, &self.before).map_err(|message| PluginFailure {
            message,
            receipt: Box::new(receipt.clone()),
            rollback_error: None,
        })?;
        Ok(receipt)
    }

    pub fn kept_plugin(&self) -> bool {
        self.original_receipt.target.installation(&self.before).is_some()
    }

    pub fn kept_marketplace(&self) -> bool {
        self.original_receipt.target.market(&self.before).is_some() || self.kept_plugin()
    }
}

/// Withdraws the dedicated product even when an interrupted command has no
/// result checkpoint. Its source is still checked before any removal.
pub fn unapply(host: &mut impl PluginHost, previous: &PluginReceipt) -> Result<PluginReceipt, PluginFailure> {
    let mut receipt = previous.clone();
    let empty = PluginState { installations: vec![], marketplaces: vec![] };
    baseline(host, &mut receipt, &empty).map_err(|message| PluginFailure {
        message,
        receipt: Box::new(receipt.clone()),
        rollback_error: None,
    })?;
    Ok(receipt)
}

/// The reserved product can be withdrawn without a prior install receipt.
pub fn unapply_managed(
    host: &mut impl PluginHost,
    plan: &ClaudePlan,
    previous: Option<&PluginReceipt>,
) -> Result<PluginReceipt, PluginFailure> {
    let target = Target::new(plan).map_err(|message| PluginFailure {
        message,
        receipt: Box::new(PluginReceipt::empty(Target {
            marketplace: plan.marketplace.clone(),
            name: plan.marketplace_name.into(),
            plugin: plan.plugin.into(),
        })),
        rollback_error: None,
    })?;
    let receipt = previous.cloned().unwrap_or_else(|| PluginReceipt::empty(target.clone()));
    if receipt.target != target {
        return Err(PluginFailure {
            message: "Claude receipt belongs to a different marketplace".into(),
            receipt: Box::new(receipt),
            rollback_error: None,
        });
    }
    unapply(host, &receipt)
}

fn capture_current(receipt: &mut PluginReceipt, state: &PluginState) {
    receipt.marketplace = receipt.target.market(state).cloned();
    receipt.plugin = receipt.target.installation(state).cloned();
    receipt.marketplace_added = receipt.marketplace.is_some();
    receipt.plugin_added = receipt.plugin.is_some();
    if receipt.plugin.is_some() {
        receipt.unconfirmed_install = false;
    }
}

fn mutate(host: &mut impl PluginHost, receipt: &mut PluginReceipt, action: Mutation) -> Result<PluginState, String> {
    receipt.pending = Some(action);
    receipt.pending_observed = false;
    if action == Mutation::InstallPlugin {
        receipt.unconfirmed_install = true;
    }
    host.checkpoint(receipt)?;
    let result = host.run(&receipt.target.command(action));
    let state = snapshot(host)?;
    capture_current(receipt, &state);
    receipt.pending_observed = true;
    host.checkpoint(receipt)?;
    result?;
    receipt.target.preflight(&state)?;
    receipt.pending = None;
    receipt.pending_observed = false;
    host.checkpoint(receipt)?;
    Ok(state)
}

fn present(host: &mut impl PluginHost, receipt: &mut PluginReceipt, version: Option<&str>) -> Result<(), String> {
    let mut state = snapshot(host)?;
    receipt.target.plugins_preflight(&state)?;
    if receipt.target.market(&state).is_some_and(|market| receipt.target.check_market(market).is_err()) {
        if receipt.target.members(&state).any(|plugin| plugin.id != receipt.target.id()) {
            return Err("The fastctx marketplace points at another source and holds another plugin; it was left unchanged".into());
        }
        state = mutate(host, receipt, Mutation::RemoveMarketplace)?;
        if receipt.target.market(&state).is_some() {
            return Err("Claude marketplace remains after removal (another scope may still declare it)".into());
        }
    }
    if receipt.target.market(&state).is_none() {
        state = mutate(host, receipt, Mutation::AddMarketplace)?;
        receipt.target.check_market(receipt.target.market(&state).ok_or("Claude did not add the marketplace")?)?;
    }
    if receipt.target.installation(&state).is_some_and(|plugin| !plugin.errors.is_empty()) {
        state = mutate(host, receipt, Mutation::UninstallPlugin)?;
        if receipt.target.installation(&state).is_some() {
            return Err("Claude plugin remains after uninstall".into());
        }
    }
    if receipt.target.installation(&state).is_none() {
        state = mutate(host, receipt, Mutation::InstallPlugin)?;
        if receipt.target.installation(&state).is_none() {
            return Err("Claude did not install the plugin".into());
        }
    }
    if receipt.target.installation(&state).is_some_and(|plugin| !plugin.enabled) {
        state = mutate(host, receipt, Mutation::EnablePlugin)?;
    }
    // Sessions read the marketplace's plugin in place; Claude Code's record
    // of it is brought along, once for each version.
    if let Some(version) = version
        && receipt.target.installation(&state).is_some_and(|plugin| stale(plugin, receipt, version))
    {
        state = mutate(host, receipt, Mutation::UpdatePlugin)?;
        receipt.updated = Some(version.into());
    }
    receipt.target.check_plugin(receipt.target.installation(&state).ok_or("Claude plugin is missing")?)?;
    capture_current(receipt, &state);
    receipt.pending = None;
    receipt.pending_observed = false;
    receipt.unconfirmed_install = false;
    host.checkpoint(receipt)
}

fn baseline(host: &mut impl PluginHost, receipt: &mut PluginReceipt, before: &PluginState) -> Result<(), String> {
    let wanted_plugin = receipt.target.installation(before);
    let wanted_marketplace = receipt.target.market(before).is_some() || wanted_plugin.is_some();
    if let Some(wanted) = wanted_plugin {
        let mut state = snapshot(host)?;
        receipt.target.preflight(&state)?;
        if receipt.target.market(&state).is_none() {
            state = mutate(host, receipt, Mutation::AddMarketplace)?;
        }
        if receipt.target.installation(&state).is_none() {
            state = mutate(host, receipt, Mutation::InstallPlugin)?;
        }
        if receipt.target.installation(&state).is_some_and(|plugin| plugin.enabled != wanted.enabled) {
            state =
                mutate(host, receipt, if wanted.enabled { Mutation::EnablePlugin } else { Mutation::DisablePlugin })?;
        }
        if receipt.target.installation(&state).is_none_or(|plugin| plugin.enabled != wanted.enabled) {
            return Err("Claude did not restore the plugin's prior enabled state".into());
        }
        capture_current(receipt, &state);
        receipt.pending = None;
        receipt.pending_observed = false;
        return host.checkpoint(receipt);
    }
    let mut state = snapshot(host)?;
    receipt.target.preflight(&state)?;
    if receipt.target.installation(&state).is_some() {
        state = mutate(host, receipt, Mutation::UninstallPlugin)?;
        if receipt.target.installation(&state).is_some() {
            return Err("Claude plugin remains after uninstall".into());
        }
    }
    if !wanted_marketplace && receipt.target.market(&state).is_some() {
        if receipt.target.members(&state).next().is_some() {
            return Err("The fastctx marketplace contains another plugin; it was left unchanged".into());
        }
        state = mutate(host, receipt, Mutation::RemoveMarketplace)?;
        if receipt.target.market(&state).is_some() {
            return Err("Claude marketplace remains after removal (another scope may still declare it)".into());
        }
    } else if wanted_marketplace && receipt.target.market(&state).is_none() {
        state = mutate(host, receipt, Mutation::AddMarketplace)?;
        if receipt.target.market(&state).is_none() {
            return Err("Claude did not restore the marketplace".into());
        }
    }
    capture_current(receipt, &state);
    receipt.pending = None;
    receipt.pending_observed = false;
    receipt.unconfirmed_install = false;
    host.checkpoint(receipt)
}

/// Whether Claude Code records `plugin` at another version than `version`,
/// and was not already brought to it.
fn stale(plugin: &Installation, receipt: &PluginReceipt, version: &str) -> bool {
    plugin.version != version && receipt.updated.as_deref() != Some(version)
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

/// The commands withdrawing the reserved product would run against
/// `state`, as `unapply_managed` runs them: the user installation
/// uninstalled, then the marketplace removed when nothing else of it is
/// installed.
pub fn withdrawal(plan: &ClaudePlan, state: &PluginState) -> Result<Vec<PluginCommand>, String> {
    let target = Target::new(plan)?;
    target.preflight(state)?;
    let mut commands = Vec::new();
    let installed = target.installation(state).is_some();
    if installed {
        commands.push(target.command(Mutation::UninstallPlugin));
    }
    if target.market(state).is_some() {
        let others =
            target.members(state).any(|plugin| !(installed && plugin.id == target.id() && plugin.scope == "user"));
        if others {
            return Err("The fastctx marketplace contains another plugin; it was left unchanged".into());
        }
        commands.push(target.command(Mutation::RemoveMarketplace));
    }
    Ok(commands)
}
