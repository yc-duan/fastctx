//! Group hierarchy, focus navigation, and draft-value model for the configuration screen.

use crate::control::config_i18n::ConfigMessages;
use crate::control::guard_i18n::GuardMessages;
use crate::control::i18n::Messages;
use crate::control::job_i18n::JobMessages;
use crate::control::settings::{
    FastCtxSettings, MAX_REPLACE_FILE_LIMIT_MIB, MIN_REPLACE_FILE_LIMIT_MIB, Tier, ToolBudgetLevel,
    ToolBudgetPreferences, UpdateSource,
};
use crate::search_parallelism;
use crate::tui::update::UpdateMessages;

/// Stable configuration-group identifier; new groups add descriptors without changing navigation or rendering algorithms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigGroupId {
    Output,
    Guard,
    Editing,
    Extensions,
    Search,
    Update,
    Reset,
    Save,
}

/// Stable identifier for an adjustable item within a configuration group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigItemId {
    OutputTier,
    OutputGuard,
    ReplaceFileLimit,
    ReadBudget,
    GrepBudget,
    GlobBudget,
    RunBudget,
    JobOutputBudget,
    FastShell,
    JobStorageLimit,
    MaxRunningJobs,
    JobListLimit,
    SearchCpuLimit,
    UpdateAutoCheck,
    UpdateSource,
    ResetAll,
    SaveAll,
}

/// Parent or child role of a configuration item within its group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigItemRole {
    Parent,
    Child { is_last: bool },
}

/// One configuration group with a parent item and zero or more dependent children.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConfigGroupSpec {
    id: ConfigGroupId,
    parent: ConfigItemId,
    children: &'static [ConfigItemId],
    standalone_items: bool,
    /// Whether this group sits outside the scrolling list, rendered in its own fixed area.
    pinned: bool,
}

impl ConfigGroupSpec {
    /// Configuration-group identifier.
    pub(crate) const fn id(self) -> ConfigGroupId {
        self.id
    }

    /// Parent item in the group.
    pub(crate) const fn parent(self) -> ConfigItemId {
        self.parent
    }

    /// Dependent child items in the group.
    pub(crate) const fn children(self) -> &'static [ConfigItemId] {
        self.children
    }

    /// Whether every item in this group is a peer rather than a parent/child hierarchy.
    pub(crate) const fn standalone_items(self) -> bool {
        self.standalone_items
    }

    /// Whether this group renders outside the scrolling list rather than within it.
    pub(crate) const fn pinned(self) -> bool {
        self.pinned
    }

    const fn item_count(self) -> usize {
        1 + self.children.len()
    }

    fn item_at(self, item_index: usize) -> ConfigItemId {
        if item_index == 0 {
            self.parent
        } else {
            self.children[item_index - 1]
        }
    }
}

const OUTPUT_CHILDREN: [ConfigItemId; 5] = [
    ConfigItemId::ReadBudget,
    ConfigItemId::GrepBudget,
    ConfigItemId::GlobBudget,
    ConfigItemId::RunBudget,
    ConfigItemId::JobOutputBudget,
];

const EXTENSION_CHILDREN: [ConfigItemId; 3] = [
    ConfigItemId::JobStorageLimit,
    ConfigItemId::MaxRunningJobs,
    ConfigItemId::JobListLimit,
];

const UPDATE_CHILDREN: [ConfigItemId; 1] = [ConfigItemId::UpdateSource];

const CONFIG_GROUPS: [ConfigGroupSpec; 8] = [
    ConfigGroupSpec {
        id: ConfigGroupId::Output,
        parent: ConfigItemId::OutputTier,
        children: &OUTPUT_CHILDREN,
        standalone_items: false,
        pinned: false,
    },
    // The provider guard is its own section rather than an output child: it is not another
    // budget knob but a compatibility protection that overrides the whole output group.
    ConfigGroupSpec {
        id: ConfigGroupId::Guard,
        parent: ConfigItemId::OutputGuard,
        children: &[],
        standalone_items: true,
        pinned: false,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Editing,
        parent: ConfigItemId::ReplaceFileLimit,
        children: &[],
        standalone_items: true,
        pinned: false,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Extensions,
        parent: ConfigItemId::FastShell,
        children: &EXTENSION_CHILDREN,
        standalone_items: true,
        pinned: false,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Search,
        parent: ConfigItemId::SearchCpuLimit,
        children: &[],
        standalone_items: true,
        pinned: false,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Update,
        parent: ConfigItemId::UpdateAutoCheck,
        children: &UPDATE_CHILDREN,
        standalone_items: true,
        pinned: false,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Reset,
        parent: ConfigItemId::ResetAll,
        children: &[],
        standalone_items: true,
        pinned: false,
    },
    // Save is pinned below the list rather than scrolling with it: the one action that ends the
    // editing session must never be somewhere the reader has to scroll to find.
    ConfigGroupSpec {
        id: ConfigGroupId::Save,
        parent: ConfigItemId::SaveAll,
        children: &[],
        standalone_items: true,
        pinned: true,
    },
];

/// Returns every configuration group in UI order.
pub(crate) const fn groups() -> &'static [ConfigGroupSpec] {
    &CONFIG_GROUPS
}

/// Returns a configuration-group descriptor by identifier.
pub(crate) fn group_spec(group: ConfigGroupId) -> ConfigGroupSpec {
    groups()
        .iter()
        .copied()
        .find(|candidate| candidate.id() == group)
        .expect("every config entry belongs to a declared group")
}

/// Configuration-group title, or `None` for a group that renders without a heading.
pub(crate) fn group_title(
    group: ConfigGroupId,
    messages: &Messages,
    config_messages: &ConfigMessages,
    guard_messages: &GuardMessages,
    updates: &UpdateMessages,
) -> Option<&'static str> {
    Some(match group {
        ConfigGroupId::Output => messages.config_title,
        ConfigGroupId::Guard => guard_messages.section_title,
        ConfigGroupId::Editing => config_messages.editing_group_title,
        ConfigGroupId::Extensions => messages.extensions_title,
        ConfigGroupId::Search => config_messages.search_group_title,
        ConfigGroupId::Update => updates.page_title,
        ConfigGroupId::Reset => config_messages.reset_group_title,
        // The save button already says what it is; a heading above it would only repeat it.
        ConfigGroupId::Save => return None,
    })
}

/// Configuration-item label; tool identifiers remain English by contract.
pub(crate) fn item_label(
    item: ConfigItemId,
    messages: &Messages,
    config_messages: &ConfigMessages,
    guard_messages: &GuardMessages,
    jobs: &JobMessages,
    updates: &UpdateMessages,
) -> &'static str {
    match item {
        ConfigItemId::OutputTier => messages.tier_label,
        ConfigItemId::OutputGuard => guard_messages.label,
        ConfigItemId::ReplaceFileLimit => config_messages.replace_limit_label,
        ConfigItemId::ReadBudget => "inspect_local_file",
        ConfigItemId::GrepBudget => "grep",
        ConfigItemId::GlobBudget => "glob",
        ConfigItemId::RunBudget => "run",
        ConfigItemId::JobOutputBudget => "job_output",
        ConfigItemId::FastShell => messages.fastshell_label,
        ConfigItemId::JobStorageLimit => jobs.storage_label,
        ConfigItemId::MaxRunningJobs => jobs.running_limit_label,
        ConfigItemId::JobListLimit => jobs.job_list_limit_label,
        ConfigItemId::SearchCpuLimit => config_messages.cpu_limit_label,
        ConfigItemId::UpdateAutoCheck => updates.auto_check_label,
        ConfigItemId::UpdateSource => updates.source_label,
        ConfigItemId::ResetAll => config_messages.reset_all_label,
        ConfigItemId::SaveAll => config_messages.save_all_label,
    }
}

/// Currently focused item and its hierarchy context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConfigEntry {
    pub(crate) group: ConfigGroupId,
    pub(crate) item: ConfigItemId,
    pub(crate) role: ConfigItemRole,
}

/// Configuration focus expressed as group and in-group indices instead of flattened magic numbers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ConfigCursor {
    group_index: usize,
    item_index: usize,
}

impl ConfigCursor {
    /// Currently focused item.
    pub(crate) fn entry(self) -> ConfigEntry {
        self.entry_in(groups())
    }

    /// Moves cyclically to the previous item; group titles are not focusable.
    pub(crate) fn previous(self) -> Self {
        self.previous_in(groups())
    }

    /// Moves cyclically to the next item, entering the next group's parent across a boundary.
    pub(crate) fn next(self) -> Self {
        self.next_in(groups())
    }

    /// Jumps to the previous group's parent for Shift-Tab navigation.
    pub(crate) fn previous_group(self) -> Self {
        let group_index = if self.group_index == 0 {
            groups().len() - 1
        } else {
            self.group_index - 1
        };
        Self {
            group_index,
            item_index: 0,
        }
    }

    /// Jumps to the next group's parent for Tab navigation.
    pub(crate) fn next_group(self) -> Self {
        Self {
            group_index: (self.group_index + 1) % groups().len(),
            item_index: 0,
        }
    }

    fn entry_in(self, groups: &[ConfigGroupSpec]) -> ConfigEntry {
        let group = groups[self.group_index];
        let item = group.item_at(self.item_index);
        let role = if group.standalone_items || self.item_index == 0 {
            ConfigItemRole::Parent
        } else {
            ConfigItemRole::Child {
                is_last: self.item_index == group.item_count() - 1,
            }
        };
        ConfigEntry {
            group: group.id(),
            item,
            role,
        }
    }

    fn previous_in(self, groups: &[ConfigGroupSpec]) -> Self {
        if self.item_index > 0 {
            return Self {
                group_index: self.group_index,
                item_index: self.item_index - 1,
            };
        }
        let group_index = if self.group_index == 0 {
            groups.len() - 1
        } else {
            self.group_index - 1
        };
        Self {
            group_index,
            item_index: groups[group_index].item_count() - 1,
        }
    }

    fn next_in(self, groups: &[ConfigGroupSpec]) -> Self {
        if self.item_index + 1 < groups[self.group_index].item_count() {
            return Self {
                group_index: self.group_index,
                item_index: self.item_index + 1,
            };
        }
        Self {
            group_index: (self.group_index + 1) % groups.len(),
            item_index: 0,
        }
    }
}

/// Flattened configuration-list row that keeps group titles distinct from focusable items.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigListRow {
    Group(ConfigGroupId),
    Item(ConfigEntry),
}

/// Expands group titles and items in UI order for a shared viewport/rendering row model.
///
/// Pinned groups are absent: they render in their own area, so putting them here would draw them
/// twice and let the viewport scroll something that is supposed to stay put.
pub(crate) fn list_rows() -> &'static [ConfigListRow] {
    static ROWS: std::sync::OnceLock<Vec<ConfigListRow>> = std::sync::OnceLock::new();
    ROWS.get_or_init(build_list_rows)
}

fn build_list_rows() -> Vec<ConfigListRow> {
    let mut rows = Vec::new();
    for group in groups().iter().filter(|group| !group.pinned()) {
        rows.push(ConfigListRow::Group(group.id()));
        rows.push(ConfigListRow::Item(ConfigEntry {
            group: group.id(),
            item: group.parent(),
            role: ConfigItemRole::Parent,
        }));
        for (index, item) in group.children().iter().copied().enumerate() {
            rows.push(ConfigListRow::Item(ConfigEntry {
                group: group.id(),
                item,
                role: if group.standalone_items() {
                    ConfigItemRole::Parent
                } else {
                    ConfigItemRole::Child {
                        is_last: index + 1 == group.children().len(),
                    }
                },
            }));
        }
    }
    rows
}

/// Every focusable item in UI order, pinned groups included.
pub(crate) fn all_items() -> impl Iterator<Item = ConfigItemId> {
    groups()
        .iter()
        .flat_map(|group| std::iter::once(group.parent()).chain(group.children().iter().copied()))
}

/// Cursor pointing at one item by identity.
///
/// Anything that needs to reach a specific setting goes through here rather than walking to it by
/// position. A relative path breaks every time an unrelated item is added elsewhere in the list,
/// which turns an ordinary addition into a wall of red that says nothing about it.
#[cfg(test)]
pub(crate) fn cursor_for(item: ConfigItemId) -> ConfigCursor {
    let mut cursor = ConfigCursor::default();
    for _ in 0..all_items().count() {
        if cursor.entry().item == item {
            return cursor;
        }
        cursor = cursor.next();
    }
    panic!("{item:?} is not reachable from the default cursor");
}

/// Row index of the current focus in the flattened list, or `None` while a pinned item has focus.
pub(crate) fn focused_row(cursor: ConfigCursor) -> Option<usize> {
    let focused = cursor.entry();
    list_rows()
        .iter()
        .position(|row| matches!(row, ConfigListRow::Item(entry) if *entry == focused))
}

/// Bounded configuration viewport whose offset names the first real row, excluding more-markers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ConfigViewport {
    offset: usize,
}

/// Content window and edge markers for one render.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ConfigViewportWindow {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) show_above: bool,
    pub(crate) show_below: bool,
}

impl ConfigViewport {
    /// Keeps the focused row visible and reserves edge-marker rows when space permits.
    pub(crate) fn window(
        &mut self,
        cursor: ConfigCursor,
        total_rows: usize,
        visible_rows: usize,
    ) -> ConfigViewportWindow {
        if total_rows == 0 || visible_rows == 0 {
            self.offset = 0;
            return ConfigViewportWindow::default();
        }
        // Focus on a pinned item anchors the list at its end: the reader walked off the bottom to
        // get there, so the rows they just left are the ones that should stay on screen.
        let focused = focused_row(cursor)
            .unwrap_or(total_rows - 1)
            .min(total_rows - 1);
        let mut best: Option<(usize, usize, usize, ConfigViewportWindow)> = None;

        // For each start, a shorter fitting window can never beat its longest one.
        for start in focused.saturating_add(1).saturating_sub(visible_rows)..=focused {
            let capacity = visible_rows.saturating_sub(usize::from(start > 0));
            let end = if total_rows - start <= capacity {
                total_rows
            } else {
                start.saturating_add(capacity.saturating_sub(1))
            };
            if end <= focused {
                continue;
            }
            let show_above = start > 0;
            let show_below = end < total_rows;
            let rendered_rows = end - start + usize::from(show_above) + usize::from(show_below);
            if rendered_rows > visible_rows {
                continue;
            }
            let content_rows = end - start;
            let movement = start.abs_diff(self.offset);
            let center = start + content_rows.saturating_sub(1) / 2;
            let focus_distance = focused.abs_diff(center);
            let window = ConfigViewportWindow {
                start,
                end,
                show_above,
                show_below,
            };
            let replace =
                best.as_ref()
                    .is_none_or(|(best_content, best_movement, best_distance, _)| {
                        content_rows > *best_content
                            || (content_rows == *best_content && movement < *best_movement)
                            || (content_rows == *best_content
                                && movement == *best_movement
                                && focus_distance < *best_distance)
                    });
            if replace {
                best = Some((content_rows, movement, focus_distance, window));
            }
        }

        let window = best.map_or_else(
            || ConfigViewportWindow {
                start: focused,
                end: focused + 1,
                show_above: false,
                show_below: false,
            },
            |(_, _, _, window)| window,
        );
        self.offset = window.start;
        window
    }
}

/// One per-tool budget as the configuration screen needs to present it.
///
/// The absolute ceiling travels with the share because a percentage alone is a leaky abstraction:
/// the same number means a different amount of output at every tier, and the tool budget the user
/// is actually reasoning about is the token count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BudgetValue {
    /// Share in effect, whether set explicitly or inherited from the tier.
    pub(crate) level: ToolBudgetLevel,
    /// Whether this share was set explicitly rather than following the tier's default.
    pub(crate) explicit: bool,
    /// Token ceiling this share resolves to under the drafted tier.
    pub(crate) tokens: usize,
}

/// Typed view of a configuration item's current value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigValue {
    Tier(Tier),
    GuardedTier(Tier),
    Budget(BudgetValue),
    Toggle(bool),
    Number(u64),
    ReplaceLimit(i64),
    CpuLimit(Option<i64>),
    Source(UpdateSource),
    Action,
}

/// Output-group draft with the tier as parent and five long-output tool budgets as children.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutputConfigDraft {
    pub(crate) tier: Tier,
    pub(crate) budgets: ToolBudgetPreferences,
}

impl OutputConfigDraft {
    /// Mutable slot for one tool's override, or `None` for items that are not budgets.
    fn budget_slot(&mut self, item: ConfigItemId) -> Option<&mut Option<ToolBudgetLevel>> {
        Some(match item {
            ConfigItemId::ReadBudget => &mut self.budgets.read,
            ConfigItemId::GrepBudget => &mut self.budgets.grep,
            ConfigItemId::GlobBudget => &mut self.budgets.glob,
            ConfigItemId::RunBudget => &mut self.budgets.run,
            ConfigItemId::JobOutputBudget => &mut self.budgets.job_output,
            _ => return None,
        })
    }

    /// Stored override for one tool, or `None` when it follows the tier.
    const fn budget_override(self, item: ConfigItemId) -> Option<ToolBudgetLevel> {
        match item {
            ConfigItemId::ReadBudget => self.budgets.read,
            ConfigItemId::GrepBudget => self.budgets.grep,
            ConfigItemId::GlobBudget => self.budgets.glob,
            ConfigItemId::RunBudget => self.budgets.run,
            ConfigItemId::JobOutputBudget => self.budgets.job_output,
            _ => None,
        }
    }

    /// Tier default for one tool.
    const fn budget_default(self, item: ConfigItemId, guarded: bool) -> ToolBudgetLevel {
        if guarded {
            return ToolBudgetLevel::Inherit;
        }
        let defaults = self.tier.default_budgets();
        match item {
            ConfigItemId::GrepBudget => defaults.grep,
            ConfigItemId::GlobBudget => defaults.glob,
            ConfigItemId::RunBudget => defaults.run,
            ConfigItemId::JobOutputBudget => defaults.job_output,
            _ => defaults.read,
        }
    }

    /// Share, provenance, and absolute ceiling for one budget item.
    fn budget_value(self, item: ConfigItemId, guarded: bool) -> BudgetValue {
        let stored = self.budget_override(item);
        let level = stored.unwrap_or_else(|| self.budget_default(item, guarded));
        let global = if guarded {
            crate::control::provider::GUARDED_FASTCTX_BUDGET
        } else {
            self.tier.fastctx_budget()
        };
        BudgetValue {
            level,
            explicit: stored.is_some(),
            tokens: level.ceiling(global),
        }
    }
}

/// Discardable draft spanning every configuration group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConfigDraft {
    pub(crate) output: OutputConfigDraft,
    pub(crate) output_guard_enabled: bool,
    pub(crate) replace_file_limit_mib: i64,
    pub(crate) fastshell_enabled: bool,
    pub(crate) job_storage_limit_mib: u64,
    pub(crate) max_running_jobs: u64,
    pub(crate) job_list_limit: u64,
    pub(crate) search_max_cpu_cores: Option<i64>,
    pub(crate) update_auto_check: bool,
    pub(crate) update_source: UpdateSource,
}

impl ConfigDraft {
    /// Builds a draft from saved settings; Esc discards it and Enter writes it back.
    pub(crate) const fn from_settings(settings: &FastCtxSettings) -> Self {
        Self {
            output: OutputConfigDraft {
                tier: settings.tier,
                budgets: settings.tool_budgets,
            },
            output_guard_enabled: settings.output_guard.enabled,
            replace_file_limit_mib: settings.replace.max_file_size_mib,
            fastshell_enabled: settings.fastshell.enabled,
            job_storage_limit_mib: settings.fastshell.job_storage_limit_mib,
            max_running_jobs: settings.fastshell.max_running_jobs,
            job_list_limit: settings.fastshell.job_list_limit,
            search_max_cpu_cores: settings.search.max_cpu_cores,
            update_auto_check: settings.update.auto_check,
            update_source: settings.update.source,
        }
    }

    /// Maps the draft back to existing persisted fields without changing serialized key semantics.
    pub(crate) fn apply_to(self, settings: &mut FastCtxSettings) {
        settings.tier = self.output.tier;
        settings.tool_budgets = self.output.budgets;
        settings.output_guard.enabled = self.output_guard_enabled;
        settings.replace.max_file_size_mib = self.replace_file_limit_mib;
        settings.fastshell.enabled = self.fastshell_enabled;
        settings.fastshell.job_storage_limit_mib = self.job_storage_limit_mib;
        settings.fastshell.max_running_jobs = self.max_running_jobs;
        settings.fastshell.job_list_limit = self.job_list_limit;
        settings.search.max_cpu_cores = self.search_max_cpu_cores;
        settings.update.auto_check = self.update_auto_check;
        settings.update.source = self.update_source;
        settings.fastedit.enabled = false;
    }

    /// Returns one value under the environment-derived Guarded mode.
    pub(crate) fn value_with_guard(self, item: ConfigItemId, guarded: bool) -> ConfigValue {
        match item {
            ConfigItemId::OutputTier if guarded => ConfigValue::GuardedTier(self.output.tier),
            ConfigItemId::OutputTier => ConfigValue::Tier(self.output.tier),
            ConfigItemId::OutputGuard => ConfigValue::Toggle(self.output_guard_enabled),
            ConfigItemId::ReplaceFileLimit => {
                ConfigValue::ReplaceLimit(self.replace_file_limit_mib)
            }
            ConfigItemId::ReadBudget
            | ConfigItemId::GrepBudget
            | ConfigItemId::GlobBudget
            | ConfigItemId::RunBudget
            | ConfigItemId::JobOutputBudget => {
                ConfigValue::Budget(self.output.budget_value(item, guarded))
            }
            ConfigItemId::FastShell => ConfigValue::Toggle(self.fastshell_enabled),
            ConfigItemId::JobStorageLimit => ConfigValue::Number(self.job_storage_limit_mib),
            ConfigItemId::MaxRunningJobs => ConfigValue::Number(self.max_running_jobs),
            ConfigItemId::JobListLimit => ConfigValue::Number(self.job_list_limit),
            ConfigItemId::SearchCpuLimit => ConfigValue::CpuLimit(self.search_max_cpu_cores),
            ConfigItemId::UpdateAutoCheck => ConfigValue::Toggle(self.update_auto_check),
            ConfigItemId::UpdateSource => ConfigValue::Source(self.update_source),
            ConfigItemId::ResetAll | ConfigItemId::SaveAll => ConfigValue::Action,
        }
    }

    /// Whether this item alone carries an edit the saved settings do not have yet.
    pub(crate) fn item_changed(self, saved: Self, item: ConfigItemId, guarded: bool) -> bool {
        match item {
            // A share that follows the tier is not itself edited when the tier moves, even though
            // the numbers it resolves to move with it. Compare what a save would write.
            ConfigItemId::ReadBudget
            | ConfigItemId::GrepBudget
            | ConfigItemId::GlobBudget
            | ConfigItemId::RunBudget
            | ConfigItemId::JobOutputBudget => {
                self.output.budget_override(item) != saved.output.budget_override(item)
            }
            _ => self.value_with_guard(item, guarded) != saved.value_with_guard(item, guarded),
        }
    }

    /// Adjusts one item while respecting an environment-derived tier lock.
    pub(crate) fn adjust_with_guard(&mut self, item: ConfigItemId, forward: bool, guarded: bool) {
        match item {
            ConfigItemId::OutputTier if !guarded => {
                self.output.tier = if forward {
                    self.output.tier.next()
                } else {
                    self.output.tier.previous()
                };
            }
            ConfigItemId::OutputTier | ConfigItemId::OutputGuard => {}
            ConfigItemId::ReplaceFileLimit => cycle_i64_preset(
                &mut self.replace_file_limit_mib,
                &[64, 128, 256, 512, 1_024, 2_048, 4_096],
                forward,
            ),
            ConfigItemId::ReadBudget
            | ConfigItemId::GrepBudget
            | ConfigItemId::GlobBudget
            | ConfigItemId::RunBudget
            | ConfigItemId::JobOutputBudget => self.cycle_budget(item, forward),
            ConfigItemId::FastShell => self.fastshell_enabled = !self.fastshell_enabled,
            ConfigItemId::JobStorageLimit => {
                cycle_preset(
                    &mut self.job_storage_limit_mib,
                    &[512, 1_024, 2_048, 4_096],
                    forward,
                );
            }
            ConfigItemId::MaxRunningJobs => {
                cycle_preset(&mut self.max_running_jobs, &[64, 128, 256, 512], forward);
            }
            ConfigItemId::JobListLimit => {
                cycle_preset(&mut self.job_list_limit, &[10, 20, 50, 100], forward);
            }
            ConfigItemId::SearchCpuLimit => cycle_cpu_limit(
                &mut self.search_max_cpu_cores,
                search_parallelism::detected_available(),
                forward,
            ),
            ConfigItemId::UpdateAutoCheck => self.update_auto_check = !self.update_auto_check,
            ConfigItemId::UpdateSource => {
                self.update_source = if forward {
                    self.update_source.next()
                } else {
                    self.update_source.previous()
                };
            }
            ConfigItemId::ResetAll | ConfigItemId::SaveAll => {}
        }
    }

    /// Accepts a validated editor result without touching other unsaved settings.
    pub(crate) fn set_search_cpu_limit(&mut self, configured: Option<i64>) {
        self.search_max_cpu_cores = configured;
    }

    /// Sets the provider guard only after the caller has completed any required confirmation.
    pub(crate) fn set_output_guard(&mut self, enabled: bool) {
        self.output_guard_enabled = enabled;
    }

    /// Accepts a validated budget editor result; `None` returns the tool to its tier default.
    pub(crate) fn set_tool_budget(&mut self, item: ConfigItemId, level: Option<ToolBudgetLevel>) {
        if let Some(slot) = self.output.budget_slot(item) {
            *slot = level;
        }
    }

    /// Resolves what a candidate editor entry would become, so the editor can show its effect
    /// before anything is saved. Routed through the real setter so the preview cannot drift from
    /// what submitting actually does.
    /// Resolves an editor entry under the environment-derived Guarded mode.
    pub(crate) fn preview_tool_budget_with_guard(
        self,
        item: ConfigItemId,
        level: Option<ToolBudgetLevel>,
        guarded: bool,
    ) -> BudgetValue {
        let mut draft = self;
        draft.set_tool_budget(item, level);
        draft.output.budget_value(item, guarded)
    }

    /// Moves one budget to the next coarse stop on the arrow-key cycle.
    fn cycle_budget(&mut self, item: ConfigItemId, forward: bool) {
        let stepped = cycle_budget_stop(self.output.budget_override(item), forward);
        self.set_tool_budget(item, stepped);
    }
}

/// Coarse stops the arrow keys walk. Single-point precision belongs in the editor: reaching an
/// arbitrary share by arrow key took dozens of presses, and the four quarters are what anyone
/// picking a share by feel actually wants.
const BUDGET_STOPS: [u8; 4] = [25, 50, 75, 100];

/// Advances a stored override to its neighbouring stop, where `None` is `auto`.
///
/// An off-grid share entered in the editor snaps to the stop it is heading towards rather than
/// jumping to a fixed end, so an arrow key never undoes more of a deliberate value than the one
/// press implies. `auto` sits past the full share, which is also where running off either end of
/// the grid lands — that is what closes the cycle.
fn cycle_budget_stop(current: Option<ToolBudgetLevel>, forward: bool) -> Option<ToolBudgetLevel> {
    let stop = |percent: u8| {
        ToolBudgetLevel::from_percent(percent).expect("every budget stop is a legal share")
    };
    let Some(level) = current else {
        return Some(stop(if forward { BUDGET_STOPS[0] } else { 100 }));
    };
    let percent = level.percent();
    if forward {
        BUDGET_STOPS
            .iter()
            .copied()
            .find(|candidate| *candidate > percent)
            .map(stop)
    } else {
        BUDGET_STOPS
            .iter()
            .copied()
            .rev()
            .find(|candidate| *candidate < percent)
            .map(stop)
    }
}

fn cycle_preset(value: &mut u64, presets: &[u64], forward: bool) {
    let next = if let Some(index) = presets.iter().position(|preset| preset == value) {
        if forward {
            presets[(index + 1) % presets.len()]
        } else {
            presets[(index + presets.len() - 1) % presets.len()]
        }
    } else if forward {
        presets
            .iter()
            .copied()
            .find(|preset| preset > value)
            .unwrap_or(presets[0])
    } else {
        presets
            .iter()
            .copied()
            .rev()
            .find(|preset| preset < value)
            .unwrap_or(*presets.last().expect("job presets are non-empty"))
    };
    *value = next;
}

fn cycle_i64_preset(value: &mut i64, presets: &[i64], forward: bool) {
    debug_assert_eq!(presets.first(), Some(&MIN_REPLACE_FILE_LIMIT_MIB));
    debug_assert_eq!(presets.last(), Some(&MAX_REPLACE_FILE_LIMIT_MIB));
    let next = if let Some(index) = presets.iter().position(|preset| preset == value) {
        if forward {
            presets[(index + 1) % presets.len()]
        } else {
            presets[(index + presets.len() - 1) % presets.len()]
        }
    } else if forward {
        presets
            .iter()
            .copied()
            .find(|preset| preset > value)
            .unwrap_or(presets[0])
    } else {
        presets
            .iter()
            .copied()
            .rev()
            .find(|preset| preset < value)
            .unwrap_or(*presets.last().expect("replace presets are non-empty"))
    };
    *value = next;
}

fn cycle_cpu_limit(value: &mut Option<i64>, maximum: usize, forward: bool) {
    let middle = (maximum / 2).max(1) as i64;
    let maximum = maximum as i64;
    let mut presets = vec![None, Some(1)];
    if middle > 1 && middle < maximum {
        presets.push(Some(middle));
    }
    if maximum > 1 {
        presets.push(Some(maximum));
    }
    let current = presets.iter().position(|preset| preset == value);
    *value = match (current, forward) {
        (Some(index), true) => presets[(index + 1) % presets.len()],
        (Some(index), false) => presets[(index + presets.len() - 1) % presets.len()],
        (None, true) => Some(1),
        (None, false) => None,
    };
}
