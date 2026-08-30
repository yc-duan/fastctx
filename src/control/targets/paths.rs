//! Platform-aware user configuration and guidance paths for agent targets.

use super::AgentTarget;
use crate::control::paths::ControlPaths;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetPaths {
    pub config: PathBuf,
    pub guidance: PathBuf,
}

impl TargetPaths {
    pub fn resolve(control: &ControlPaths, target: AgentTarget) -> Result<Self, String> {
        Self::resolve_with_xdg(
            control,
            target,
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        )
    }

    pub(crate) fn resolve_with_xdg(
        control: &ControlPaths,
        target: AgentTarget,
        xdg_config_home: Option<PathBuf>,
    ) -> Result<Self, String> {
        require_absolute(&control.home, "user home")?;
        if target == AgentTarget::Codex {
            require_absolute_or_test_relative(&control.codex_dir, "Codex profile")?;
            return Ok(Self {
                config: control.codex_config.clone(),
                guidance: control.codex_agents.clone(),
            });
        }
        let home = &control.home;
        let paths = match target {
            AgentTarget::Codex => unreachable!(),
            AgentTarget::ClaudeCode => Self {
                config: home.join(".claude.json"),
                guidance: home.join(".claude").join("CLAUDE.md"),
            },
            AgentTarget::Cursor => Self {
                config: home.join(".cursor").join("mcp.json"),
                guidance: home.join(".cursor").join("rules").join("fastctx.mdc"),
            },
            AgentTarget::VscodeCopilot => Self {
                config: home.join(".copilot").join("mcp-config.json"),
                guidance: home
                    .join(".copilot")
                    .join("instructions")
                    .join("fastctx.instructions.md"),
            },
            AgentTarget::Opencode => Self {
                config: home.join(".config").join("opencode").join("opencode.json"),
                guidance: home.join(".config").join("opencode").join("AGENTS.md"),
            },
            AgentTarget::Antigravity => Self {
                config: home.join(".gemini").join("config").join("mcp_config.json"),
                guidance: home.join(".gemini").join("GEMINI.md"),
            },
            AgentTarget::Trae => Self {
                config: trae_config_path(home, xdg_config_home.as_deref())?,
                guidance: home.join(".trae-cn").join("rules").join("fastctx.md"),
            },
            AgentTarget::Zcode => Self {
                config: home.join(".zcode").join("cli").join("config.json"),
                guidance: home.join(".zcode").join("AGENTS.md"),
            },
            AgentTarget::Qoder => Self {
                config: home.join(".qoder").join("settings.json"),
                guidance: home.join(".qoder").join("AGENTS.md"),
            },
        };
        Ok(paths)
    }
}

fn require_absolute(path: &Path, label: &str) -> Result<(), String> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(format!(
            "Cannot resolve agent paths because the {label} path {} is not absolute.",
            crate::paths::display_path(path)
        ))
    }
}

fn require_absolute_or_test_relative(path: &Path, label: &str) -> Result<(), String> {
    if path.is_absolute() || cfg!(test) {
        Ok(())
    } else {
        require_absolute(path, label)
    }
}

#[cfg(windows)]
fn trae_config_path(home: &Path, _xdg: Option<&Path>) -> Result<PathBuf, String> {
    Ok(home
        .join("AppData")
        .join("Roaming")
        .join("trae_cli")
        .join("trae_cli.yaml"))
}

#[cfg(target_os = "macos")]
fn trae_config_path(home: &Path, _xdg: Option<&Path>) -> Result<PathBuf, String> {
    Ok(home
        .join("Library")
        .join("Application Support")
        .join("trae_cli")
        .join("trae_cli.yaml"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn trae_config_path(home: &Path, xdg: Option<&Path>) -> Result<PathBuf, String> {
    let base = match xdg {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => {
            return Err(format!(
                "XDG_CONFIG_HOME must be absolute to resolve TraeCode CLI, got {}.",
                crate::paths::display_path(path)
            ));
        }
        None => home.join(".config"),
    };
    Ok(base.join("trae_cli").join("trae_cli.yaml"))
}
