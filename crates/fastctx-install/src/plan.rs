//! What the World writes into each host's configuration: the same values a
//! World-started session is started with, from the same constants.

use std::path::{Path, PathBuf};

use fastctx_hands::run::window::BUDGET;
use fastctx_hosts::claude;
use fastctx_hosts::codex::{STARTUP_TIMEOUT, TOOL_TIMEOUT, output_limit};

/// The World's server in a Codex configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexPlan {
    /// The absolute path Codex starts the server from: the stable copy of
    /// fastctx, which an update replaces in place.
    pub command: PathBuf,
    pub args: Vec<String>,
    pub startup_timeout_sec: u64,
    pub tool_timeout_sec: u64,
    /// For each World tool, and for every tool through the shared root
    /// `tool_output_token_limit`: the MCP/history limit for an answer of at
    /// most `BUDGET` units, including the World's notes and script prints.
    /// Code Mode exec/wait prints have a separate per-execution output budget.
    pub output_token_limit: u64,
}

impl CodexPlan {
    pub fn new(fastctx: &Path) -> Self {
        Self {
            command: fastctx.to_path_buf(),
            args: vec!["serve".into()],
            startup_timeout_sec: STARTUP_TIMEOUT.as_secs(),
            tool_timeout_sec: TOOL_TIMEOUT.as_secs(),
            output_token_limit: output_limit(BUDGET),
        }
    }
}

/// The World's plugin and limit in Claude Code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudePlan {
    /// The local marketplace the node keeps the plugin in.
    pub marketplace: PathBuf,
    pub marketplace_name: &'static str,
    pub plugin: &'static str,
    /// The version the plugin in the marketplace is written as.
    pub plugin_version: &'static str,
    /// `env.MAX_MCP_OUTPUT_TOKENS` in the user's settings.
    pub max_mcp_output_tokens: u64,
    /// `env.CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH` in the user's settings.
    pub max_mcp_description_length: u64,
}

impl ClaudePlan {
    pub fn new(world: &Path) -> Self {
        Self {
            marketplace: claude::marketplace(world),
            marketplace_name: claude::MARKETPLACE,
            plugin: claude::PLUGIN,
            plugin_version: claude::PLUGIN_VERSION,
            max_mcp_output_tokens: claude::MAX_MCP_OUTPUT_TOKENS,
            max_mcp_description_length: claude::MAX_MCP_DESCRIPTION_LENGTH,
        }
    }
}
