//! Agent-connection list and enabled-tool draft state.

use crate::control::target_status::TargetStatus;
use crate::control::targets::AgentTarget;
use crate::server_manifest::{EnabledTools, ToolGroup, ToolManifest};

/// Data lifecycle for the stable target registry shown by the TUI.
#[derive(Clone, Debug)]
pub(crate) enum AgentListState {
    Loading,
    Ready(Vec<TargetStatus>),
    Empty,
    Error(String),
}

impl AgentListState {
    pub(crate) fn statuses(&self) -> &[TargetStatus] {
        match self {
            Self::Ready(statuses) => statuses,
            Self::Loading | Self::Empty | Self::Error(_) => &[],
        }
    }

    pub(crate) fn get(&self, target: AgentTarget) -> Option<&TargetStatus> {
        self.statuses()
            .iter()
            .find(|status| status.target == target)
    }
}

/// Stable identity for every row on the target detail page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentDetailItem {
    Inspect,
    Grep,
    Glob,
    Replace,
    ShellSuite,
    Apply,
    Disconnect,
    Doctor,
}

impl AgentDetailItem {
    pub(crate) const ALL: [Self; 8] = [
        Self::Inspect,
        Self::Grep,
        Self::Glob,
        Self::Replace,
        Self::ShellSuite,
        Self::Apply,
        Self::Disconnect,
        Self::Doctor,
    ];

    pub(crate) const fn tool_name(self) -> Option<&'static str> {
        match self {
            Self::Inspect => Some("inspect_local_file"),
            Self::Grep => Some("grep"),
            Self::Glob => Some("glob"),
            Self::Replace => Some("replace"),
            Self::ShellSuite | Self::Apply | Self::Disconnect | Self::Doctor => None,
        }
    }

    pub(crate) const fn is_toggle(self) -> bool {
        matches!(
            self,
            Self::Inspect | Self::Grep | Self::Glob | Self::Replace | Self::ShellSuite
        )
    }
}

/// Cursor whose position follows row identity rather than rendered line count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentDetailCursor {
    item: AgentDetailItem,
}

impl Default for AgentDetailCursor {
    fn default() -> Self {
        Self {
            item: AgentDetailItem::Inspect,
        }
    }
}

impl AgentDetailCursor {
    pub(crate) const fn item(self) -> AgentDetailItem {
        self.item
    }

    pub(crate) fn index(self) -> usize {
        AgentDetailItem::ALL
            .iter()
            .position(|candidate| *candidate == self.item)
            .unwrap_or(0)
    }

    pub(crate) fn previous(self) -> Self {
        let index = self.index();
        Self {
            item: AgentDetailItem::ALL[if index == 0 {
                AgentDetailItem::ALL.len() - 1
            } else {
                index - 1
            }],
        }
    }

    pub(crate) fn next(self) -> Self {
        Self {
            item: AgentDetailItem::ALL[(self.index() + 1) % AgentDetailItem::ALL.len()],
        }
    }
}

/// Persisted and in-editor enabled sets for one target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentToolDraft {
    pub(crate) target: AgentTarget,
    pub(crate) persisted: EnabledTools,
    pub(crate) current: EnabledTools,
}

impl Default for AgentToolDraft {
    fn default() -> Self {
        Self::new(AgentTarget::Codex, EnabledTools::files())
    }
}

impl AgentToolDraft {
    pub(crate) const fn new(target: AgentTarget, persisted: EnabledTools) -> Self {
        Self {
            target,
            persisted,
            current: persisted,
        }
    }

    pub(crate) fn is_dirty(self) -> bool {
        self.persisted != self.current
    }

    pub(crate) fn discard(&mut self) {
        self.current = self.persisted;
    }

    pub(crate) fn accept(&mut self, persisted: EnabledTools) {
        self.persisted = persisted;
        self.current = persisted;
    }

    pub(crate) fn enabled(self, item: AgentDetailItem) -> Option<bool> {
        if let Some(name) = item.tool_name() {
            return Some(self.current.contains(name));
        }
        (item == AgentDetailItem::ShellSuite).then(|| self.current.shell_enabled())
    }

    pub(crate) fn item_changed(self, item: AgentDetailItem) -> bool {
        match item {
            AgentDetailItem::ShellSuite => {
                self.current.shell_enabled() != self.persisted.shell_enabled()
            }
            _ => item
                .tool_name()
                .is_some_and(|name| self.current.contains(name) != self.persisted.contains(name)),
        }
    }

    /// Toggles one file tool or the complete shell lifecycle suite.
    pub(crate) fn toggle(&mut self, item: AgentDetailItem) -> Result<(), String> {
        if !item.is_toggle() {
            return Ok(());
        }
        let toggle_shell = item == AgentDetailItem::ShellSuite;
        let shell_enabled = self.current.shell_enabled();
        let file_name = item.tool_name();
        let mut names = ToolManifest::entries()
            .iter()
            .filter_map(|entry| {
                let currently_enabled = self.current.contains(entry.name);
                let enabled = if toggle_shell && entry.group == ToolGroup::Shell {
                    !shell_enabled
                } else if file_name == Some(entry.name) {
                    !currently_enabled
                } else {
                    currently_enabled
                };
                enabled.then_some(entry.name)
            })
            .collect::<Vec<_>>();
        // Keep construction order explicit even if the manifest implementation changes later.
        names.dedup();
        self.current = EnabledTools::from_names(names)?;
        Ok(())
    }
}
