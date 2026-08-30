//! Closed agent-target registry and target-specific control-plane capabilities.

mod config;
mod guidance;
mod json_config;
mod paths;
mod trae_config;

pub(crate) use config::{ConfigApplyRequest, apply_config, disconnect_config, inspect_config};
pub(crate) use guidance::{
    apply_guidance, disconnect_guidance, generated_guidance, guidance_managed_hash,
};
pub use paths::TargetPaths;

use crate::control::paths::ControlPaths;
use crate::server_manifest::EnabledTools;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// Every supported user-level agent integration in stable presentation order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentTarget {
    Codex,
    ClaudeCode,
    Cursor,
    VscodeCopilot,
    Opencode,
    Antigravity,
    Trae,
    Zcode,
    Qoder,
}

impl AgentTarget {
    pub const ALL: [Self; 9] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::Cursor,
        Self::VscodeCopilot,
        Self::Opencode,
        Self::Antigravity,
        Self::Trae,
        Self::Zcode,
        Self::Qoder,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::Cursor => "cursor",
            Self::VscodeCopilot => "vscode-copilot",
            Self::Opencode => "opencode",
            Self::Antigravity => "antigravity",
            Self::Trae => "trae",
            Self::Zcode => "zcode",
            Self::Qoder => "qoder",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex / ChatGPT",
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::VscodeCopilot => "VS Code Copilot Agent",
            Self::Opencode => "OpenCode",
            Self::Antigravity => "Antigravity",
            Self::Trae => "TraeCode CLI",
            Self::Zcode => "ZCode",
            Self::Qoder => "Qoder",
        }
    }

    pub(crate) const fn config_kind(self) -> ConfigKind {
        match self {
            Self::Codex => ConfigKind::CodexToml,
            Self::Opencode => ConfigKind::OpenCodeJsonc,
            Self::Trae => ConfigKind::TraeYaml,
            Self::Zcode => ConfigKind::NestedJsonc,
            Self::ClaudeCode
            | Self::Cursor
            | Self::VscodeCopilot
            | Self::Antigravity
            | Self::Qoder => ConfigKind::CommonJsonc,
        }
    }

    pub(crate) const fn guidance_kind(self) -> GuidanceKind {
        match self {
            Self::Cursor => GuidanceKind::CursorRule,
            Self::VscodeCopilot => GuidanceKind::CopilotInstructions,
            Self::Trae => GuidanceKind::TraeRule,
            Self::Codex
            | Self::ClaudeCode
            | Self::Opencode
            | Self::Antigravity
            | Self::Zcode
            | Self::Qoder => GuidanceKind::SharedMarkdown,
        }
    }

    pub const fn budget_policy(self) -> BudgetPolicy {
        match self {
            Self::Codex => BudgetPolicy::CodexManaged,
            Self::ClaudeCode => BudgetPolicy::ClaudeDocumented,
            Self::Opencode => BudgetPolicy::OpenCodeByteCeiling,
            _ => BudgetPolicy::UnknownHost,
        }
    }

    pub fn paths(self, control: &ControlPaths) -> Result<TargetPaths, String> {
        TargetPaths::resolve(control, self)
    }

    pub fn default_tools(self) -> EnabledTools {
        EnabledTools::files()
    }
}

impl Display for AgentTarget {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.id())
    }
}

impl FromStr for AgentTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|target| target.id() == value)
            .ok_or_else(|| {
                format!(
                    "Unknown agent target \"{value}\". Valid targets: {}.",
                    Self::ALL.map(Self::id).join(", ")
                )
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConfigKind {
    CodexToml,
    CommonJsonc,
    NestedJsonc,
    OpenCodeJsonc,
    TraeYaml,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GuidanceKind {
    SharedMarkdown,
    CursorRule,
    CopilotInstructions,
    TraeRule,
}

/// Host-output policy known for a target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BudgetPolicy {
    CodexManaged,
    ClaudeDocumented,
    OpenCodeByteCeiling,
    UnknownHost,
}

impl BudgetPolicy {
    /// Resolves the target's effective FastCtx token ceiling.
    pub const fn clamp(self, selected: usize) -> usize {
        match self {
            Self::CodexManaged => selected,
            Self::ClaudeDocumented => {
                if selected < 22_500 {
                    selected
                } else {
                    22_500
                }
            }
            Self::OpenCodeByteCeiling | Self::UnknownHost => {
                if selected < 8_500 {
                    selected
                } else {
                    8_500
                }
            }
        }
    }

    pub const fn doctor_fact(self) -> &'static str {
        match self {
            Self::CodexManaged => "Codex host token limit is managed and checked at ×1.2",
            Self::ClaudeDocumented => {
                "Claude documents a 25,000-token default; process environment is not observable"
            }
            Self::OpenCodeByteCeiling => {
                "OpenCode's 2,000-line / 50-KiB output ceiling may trigger before this token budget"
            }
            Self::UnknownHost => "host MCP output limit is unknown",
        }
    }
}
