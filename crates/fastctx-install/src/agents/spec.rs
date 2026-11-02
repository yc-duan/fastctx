//! What one agent is to Fastctx: where its user-level configuration lives,
//! how Fastctx's MCP server is registered there, its global instructions
//! file, how its own shell is turned off and its limit on a tool's answer
//! raised, and who is logged in, read from its files alone.

use std::path::{Path, PathBuf};

use fastctx_types::logins::Identity;

/// An agent connected through its user-level configuration alone
/// (`world-layers.md`, 支持尽可能多的智能体). Codex and Claude Code are not
/// among these: they take their own ways, with the workbench.
pub struct Agent {
    /// Its word, as an `AgentName`: lowercase, sixteen characters at most,
    /// never `codex` or `claude`.
    pub name: &'static str,
    /// How the log and the command line name it, such as `Gemini CLI`; the
    /// interface names it in its own words.
    pub title: &'static str,
    /// Its user-level configuration directory.
    pub home: Home,
    /// How it is known to be installed, and its version read.
    pub programs: &'static [Program],
    /// Where Fastctx's MCP server is registered, and as what.
    pub mcp: Mcp,
    /// Its global instructions file, relative to its home, where it has one.
    pub guidance: Option<&'static str>,
    /// Turning its own shell off, where it can be.
    pub shell: Option<Edit>,
    /// Raising its limits on one tool's answer, where it has them.
    pub limits: &'static [Edit],
    /// Who is logged in, read from the files of its home alone: never over
    /// the network, never by starting its program.
    pub identity: fn(&Path) -> Option<Identity>,
}

/// Where an agent's user-level configuration directory is: the first of
/// `env` that is set, joined with its subdirectory, else `default` under the
/// user's home.
pub struct Home {
    /// An environment variable and the directory under it, such as
    /// (`GEMINI_CLI_HOME`, `.gemini`); an empty subdirectory is the variable's
    /// directory itself.
    pub env: &'static [(&'static str, &'static str)],
    pub default: &'static str,
}

impl Home {
    /// The directory, as this process's environment and home place it.
    pub fn resolve(&self, user_home: &Path, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
        for (name, under) in self.env {
            if let Some(value) = var(name).filter(|value| !value.is_empty()) {
                let base = PathBuf::from(value);
                return if under.is_empty() { base } else { base.join(under) };
            }
        }
        user_home.join(self.default)
    }
}

/// How an agent is known to be installed.
pub enum Program {
    /// A command on the search path.
    Command(&'static str),
    /// An npm package installed globally.
    Npm(&'static str),
}

/// Fastctx's MCP server in an agent's configuration: always named
/// `fastctx`, in the object at `at` of `file`.
pub struct Mcp {
    /// Relative to the agent's home, such as `settings.json`.
    pub file: &'static str,
    pub format: Format,
    /// The key path of the object the servers are kept in, such as
    /// `["mcpServers"]`.
    pub at: &'static [&'static str],
    /// The entry that starts Fastctx's server.
    pub entry: fn(&Server<'_>) -> serde_json::Value,
}

/// How Fastctx's server is started: its program's absolute path, its
/// arguments, its environment.
pub struct Server<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub env: &'a [(String, String)],
}

/// The formats of configuration Fastctx edits in place, keeping the rest of
/// the file as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    /// JSON with comments and trailing commas.
    Jsonc,
}

/// One change to an agent's configuration besides its server's entry.
pub enum Edit {
    /// The value at a key path of a settings file set; taken out, what was
    /// there before is put back, or the key removed where there was none.
    Set { file: &'static str, format: Format, at: &'static [&'static str], value: fn() -> serde_json::Value },
    /// An item added to the array at a key path; taken out, that item goes.
    Add { file: &'static str, format: Format, at: &'static [&'static str], item: fn() -> serde_json::Value },
    /// A whole file of Fastctx's own, made on connecting and deleted on
    /// taking out, such as a policy file among the agent's.
    Own { file: &'static str, body: &'static str },
}

impl Edit {
    /// The file it changes, relative to the agent's home.
    pub fn file(&self) -> &'static str {
        match self {
            Self::Set { file, .. } | Self::Add { file, .. } | Self::Own { file, .. } => file,
        }
    }
}

impl Agent {
    /// Every file connecting it reads or writes, relative to its home, each
    /// once, in the order the planner takes them.
    pub fn files(&self) -> Vec<&'static str> {
        let mut files = vec![self.mcp.file];
        for edit in self.shell.iter().chain(self.limits) {
            if !files.contains(&edit.file()) {
                files.push(edit.file());
            }
        }
        if let Some(guidance) = self.guidance
            && !files.contains(&guidance)
        {
            files.push(guidance);
        }
        files
    }
}
