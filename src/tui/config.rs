//! Group hierarchy, focus navigation, and draft-value model for the configuration screen.

use crate::control::i18n::Messages;
use crate::control::job_i18n::JobMessages;
use crate::control::settings::{FastCtxSettings, Tier, ToolBudgetLevel, ToolBudgets, UpdateSource};
use crate::tui::update::UpdateMessages;

/// Stable configuration-group identifier; new groups add descriptors without changing navigation or rendering algorithms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigGroupId {
    Output,
    Extensions,
    Update,
}

/// Stable identifier for an adjustable item within a configuration group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigItemId {
    OutputTier,
    ReadBudget,
    GrepBudget,
    GlobBudget,
    RunBudget,
    JobOutputBudget,
    FastShell,
    JobStorageLimit,
    MaxRunningJobs,
    JobListLimit,
    UpdateAutoCheck,
    UpdateSource,
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

const CONFIG_GROUPS: [ConfigGroupSpec; 3] = [
    ConfigGroupSpec {
        id: ConfigGroupId::Output,
        parent: ConfigItemId::OutputTier,
        children: &OUTPUT_CHILDREN,
        standalone_items: false,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Extensions,
        parent: ConfigItemId::FastShell,
        children: &EXTENSION_CHILDREN,
        standalone_items: true,
    },
    ConfigGroupSpec {
        id: ConfigGroupId::Update,
        parent: ConfigItemId::UpdateAutoCheck,
        children: &UPDATE_CHILDREN,
        standalone_items: true,
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

/// Configuration-group title.
pub(crate) fn group_title(
    group: ConfigGroupId,
    messages: &Messages,
    updates: &UpdateMessages,
) -> &'static str {
    match group {
        ConfigGroupId::Output => messages.config_title,
        ConfigGroupId::Extensions => messages.extensions_title,
        ConfigGroupId::Update => updates.page_title,
    }
}

/// Configuration-item label; tool identifiers remain English by contract.
pub(crate) fn item_label(
    item: ConfigItemId,
    messages: &Messages,
    jobs: &JobMessages,
    updates: &UpdateMessages,
) -> &'static str {
    match item {
        ConfigItemId::OutputTier => messages.tier_label,
        ConfigItemId::ReadBudget => "read",
        ConfigItemId::GrepBudget => "grep",
        ConfigItemId::GlobBudget => "glob",
        ConfigItemId::RunBudget => "run",
        ConfigItemId::JobOutputBudget => "job_output",
        ConfigItemId::FastShell => messages.fastshell_label,
        ConfigItemId::JobStorageLimit => jobs.storage_label,
        ConfigItemId::MaxRunningJobs => jobs.running_limit_label,
        ConfigItemId::JobListLimit => jobs.job_list_limit_label,
        ConfigItemId::UpdateAutoCheck => updates.auto_check_label,
        ConfigItemId::UpdateSource => updates.source_label,
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
pub(crate) fn list_rows() -> Vec<ConfigListRow> {
    let mut rows = Vec::new();
    for group in groups() {
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

/// Row index of the current focus in the flattened list.
pub(crate) fn focused_row(cursor: ConfigCursor) -> usize {
    let focused = cursor.entry();
    list_rows()
        .iter()
        .position(|row| matches!(row, ConfigListRow::Item(entry) if *entry == focused))
        .expect("the config cursor always points at a declared item")
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
        let focused = focused_row(cursor).min(total_rows - 1);
        let mut best: Option<(usize, usize, usize, ConfigViewportWindow)> = None;

        for start in 0..=focused {
            for end in focused + 1..=total_rows {
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

/// Typed view of a configuration item's current value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigValue {
    Tier(Tier),
    Budget(ToolBudgetLevel),
    Toggle(bool),
    Number(u64),
    Source(UpdateSource),
}

/// Output-group draft with the tier as parent and five long-output tool budgets as children.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutputConfigDraft {
    pub(crate) tier: Tier,
    pub(crate) budgets: ToolBudgets,
}

/// Discardable draft spanning every configuration group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConfigDraft {
    pub(crate) output: OutputConfigDraft,
    pub(crate) fastshell_enabled: bool,
    pub(crate) job_storage_limit_mib: u64,
    pub(crate) max_running_jobs: u64,
    pub(crate) job_list_limit: u64,
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
            fastshell_enabled: settings.fastshell.enabled,
            job_storage_limit_mib: settings.fastshell.job_storage_limit_mib,
            max_running_jobs: settings.fastshell.max_running_jobs,
            job_list_limit: settings.fastshell.job_list_limit,
            update_auto_check: settings.update.auto_check,
            update_source: settings.update.source,
        }
    }

    /// Maps the draft back to existing persisted fields without changing serialized key semantics.
    pub(crate) fn apply_to(self, settings: &mut FastCtxSettings) {
        settings.tier = self.output.tier;
        settings.tool_budgets = self.output.budgets;
        settings.fastshell.enabled = self.fastshell_enabled;
        settings.fastshell.job_storage_limit_mib = self.job_storage_limit_mib;
        settings.fastshell.max_running_jobs = self.max_running_jobs;
        settings.fastshell.job_list_limit = self.job_list_limit;
        settings.update.auto_check = self.update_auto_check;
        settings.update.source = self.update_source;
        settings.fastedit.enabled = false;
    }

    /// Returns the typed current value of one item.
    pub(crate) const fn value(self, item: ConfigItemId) -> ConfigValue {
        match item {
            ConfigItemId::OutputTier => ConfigValue::Tier(self.output.tier),
            ConfigItemId::ReadBudget => ConfigValue::Budget(self.output.budgets.read),
            ConfigItemId::GrepBudget => ConfigValue::Budget(self.output.budgets.grep),
            ConfigItemId::GlobBudget => ConfigValue::Budget(self.output.budgets.glob),
            ConfigItemId::RunBudget => ConfigValue::Budget(self.output.budgets.run),
            ConfigItemId::JobOutputBudget => ConfigValue::Budget(self.output.budgets.job_output),
            ConfigItemId::FastShell => ConfigValue::Toggle(self.fastshell_enabled),
            ConfigItemId::JobStorageLimit => ConfigValue::Number(self.job_storage_limit_mib),
            ConfigItemId::MaxRunningJobs => ConfigValue::Number(self.max_running_jobs),
            ConfigItemId::JobListLimit => ConfigValue::Number(self.job_list_limit),
            ConfigItemId::UpdateAutoCheck => ConfigValue::Toggle(self.update_auto_check),
            ConfigItemId::UpdateSource => ConfigValue::Source(self.update_source),
        }
    }

    /// Adjusts the focused item cyclically in the left or right direction.
    pub(crate) fn adjust(&mut self, item: ConfigItemId, forward: bool) {
        match item {
            ConfigItemId::OutputTier => {
                self.output.tier = if forward {
                    self.output.tier.next()
                } else {
                    self.output.tier.previous()
                };
            }
            ConfigItemId::ReadBudget => cycle_budget(&mut self.output.budgets.read, forward),
            ConfigItemId::GrepBudget => cycle_budget(&mut self.output.budgets.grep, forward),
            ConfigItemId::GlobBudget => cycle_budget(&mut self.output.budgets.glob, forward),
            ConfigItemId::RunBudget => cycle_budget(&mut self.output.budgets.run, forward),
            ConfigItemId::JobOutputBudget => {
                cycle_budget(&mut self.output.budgets.job_output, forward)
            }
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
            ConfigItemId::UpdateAutoCheck => self.update_auto_check = !self.update_auto_check,
            ConfigItemId::UpdateSource => {
                self.update_source = if forward {
                    self.update_source.next()
                } else {
                    self.update_source.previous()
                };
            }
        }
    }
}

fn cycle_budget(level: &mut ToolBudgetLevel, forward: bool) {
    *level = if forward {
        level.next()
    } else {
        level.previous()
    };
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

#[cfg(test)]
mod tests {
    use super::{
        ConfigCursor, ConfigDraft, ConfigGroupId, ConfigGroupSpec, ConfigItemId, ConfigItemRole,
        ConfigViewport, OUTPUT_CHILDREN, list_rows,
    };
    use crate::control::settings::FastCtxSettings;

    #[test]
    fn cursor_preserves_group_parent_child_order_and_wraps() {
        let mut cursor = ConfigCursor::default();
        let expected = [
            (ConfigItemId::OutputTier, ConfigItemRole::Parent),
            (
                ConfigItemId::ReadBudget,
                ConfigItemRole::Child { is_last: false },
            ),
            (
                ConfigItemId::GrepBudget,
                ConfigItemRole::Child { is_last: false },
            ),
            (
                ConfigItemId::GlobBudget,
                ConfigItemRole::Child { is_last: false },
            ),
            (
                ConfigItemId::RunBudget,
                ConfigItemRole::Child { is_last: false },
            ),
            (
                ConfigItemId::JobOutputBudget,
                ConfigItemRole::Child { is_last: true },
            ),
            (ConfigItemId::FastShell, ConfigItemRole::Parent),
            (ConfigItemId::JobStorageLimit, ConfigItemRole::Parent),
            (ConfigItemId::MaxRunningJobs, ConfigItemRole::Parent),
            (ConfigItemId::JobListLimit, ConfigItemRole::Parent),
            (ConfigItemId::UpdateAutoCheck, ConfigItemRole::Parent),
            (ConfigItemId::UpdateSource, ConfigItemRole::Parent),
        ];

        for (item, role) in expected {
            let entry = cursor.entry();
            let expected_group = if matches!(
                item,
                ConfigItemId::FastShell
                    | ConfigItemId::JobStorageLimit
                    | ConfigItemId::MaxRunningJobs
                    | ConfigItemId::JobListLimit
            ) {
                ConfigGroupId::Extensions
            } else if matches!(
                item,
                ConfigItemId::UpdateAutoCheck | ConfigItemId::UpdateSource
            ) {
                ConfigGroupId::Update
            } else {
                ConfigGroupId::Output
            };
            assert_eq!(entry.group, expected_group);
            assert_eq!((entry.item, entry.role), (item, role));
            cursor = cursor.next();
        }
        assert_eq!(cursor, ConfigCursor::default());
        assert_eq!(cursor.previous().entry().item, ConfigItemId::UpdateSource);
    }

    #[test]
    fn navigation_algorithm_accepts_a_second_group_without_rewriting() {
        const SECOND_CHILDREN: [ConfigItemId; 1] = [ConfigItemId::ReadBudget];
        let groups = [
            ConfigGroupSpec {
                id: ConfigGroupId::Output,
                parent: ConfigItemId::OutputTier,
                children: &OUTPUT_CHILDREN,
                standalone_items: false,
            },
            ConfigGroupSpec {
                id: ConfigGroupId::Extensions,
                parent: ConfigItemId::GrepBudget,
                children: &SECOND_CHILDREN,
                standalone_items: false,
            },
        ];
        // Forward order: parent to children, then the next group's parent, wrapping from the final item to the first parent.
        let forward = [
            (0, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (0, 5),
            (1, 0),
            (1, 1),
        ];
        let mut cursor = ConfigCursor::default();
        for expected in forward {
            assert_eq!((cursor.group_index, cursor.item_index), expected);
            cursor = cursor.next_in(&groups);
        }
        assert_eq!(cursor, ConfigCursor::default());

        // Reverse traversal exactly mirrors forward order, including cross-group jumps and first-to-last wrapping.
        for expected in forward.into_iter().rev() {
            cursor = cursor.previous_in(&groups);
            assert_eq!((cursor.group_index, cursor.item_index), expected);
        }
        assert_eq!(cursor, ConfigCursor::default());
    }

    #[test]
    fn tab_navigation_always_lands_on_a_group_parent() {
        let output = ConfigCursor::default();
        let extensions = output.next_group();
        let update = extensions.next_group();
        assert_eq!(extensions.entry().item, ConfigItemId::FastShell);
        assert_eq!(update.entry().item, ConfigItemId::UpdateAutoCheck);
        assert_eq!(update.next_group(), output);
        assert_eq!(output.previous_group(), update);
        assert_eq!(extensions.previous_group(), output);
        assert_eq!(update.previous_group(), extensions);
    }

    #[test]
    fn job_list_page_size_cycles_all_presets_and_normalizes_custom_values() {
        let settings = FastCtxSettings::default();
        let mut draft = ConfigDraft::from_settings(&settings);
        assert_eq!(draft.job_list_limit, 20);

        for expected in [50, 100, 10, 20] {
            draft.adjust(ConfigItemId::JobListLimit, true);
            assert_eq!(draft.job_list_limit, expected);
        }
        for expected in [10, 100, 50, 20] {
            draft.adjust(ConfigItemId::JobListLimit, false);
            assert_eq!(draft.job_list_limit, expected);
        }

        draft.job_list_limit = 37;
        draft.adjust(ConfigItemId::JobListLimit, true);
        assert_eq!(draft.job_list_limit, 50);
        draft.job_list_limit = 37;
        draft.adjust(ConfigItemId::JobListLimit, false);
        assert_eq!(draft.job_list_limit, 20);
    }

    #[test]
    fn viewport_keeps_focus_visible_and_reports_both_hidden_edges() {
        let rows = list_rows();
        assert_eq!(rows.len(), 15);
        let mut viewport = ConfigViewport::default();
        let top = viewport.window(ConfigCursor::default(), rows.len(), 5);
        assert_eq!((top.start, top.end), (0, 4));
        assert!(!top.show_above);
        assert!(top.show_below);

        let mut cursor = ConfigCursor::default();
        for _ in 0..3 {
            cursor = cursor.next();
        }
        let middle = viewport.window(cursor, rows.len(), 5);
        let focused = super::focused_row(cursor);
        assert!(middle.start <= focused && focused < middle.end);
        assert!(middle.show_above);
        assert!(middle.show_below);

        while cursor.entry().item != ConfigItemId::UpdateSource {
            cursor = cursor.next();
        }
        let bottom = viewport.window(cursor, rows.len(), 5);
        let focused = super::focused_row(cursor);
        assert!(bottom.start <= focused && focused < bottom.end);
        assert!(bottom.show_above);
        assert!(!bottom.show_below);
    }
}
