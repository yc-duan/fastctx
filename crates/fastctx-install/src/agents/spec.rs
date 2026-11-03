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
    /// How it is named, such as `Qwen Code`: in the log, on the command line
    /// and in the interface.
    pub title: &'static str,
    /// Its user-level configuration directory.
    pub home: Home,
    /// What only it keeps in its home, any one of which shows it is there,
    /// where another agent keeps the same home (Qoder's CLI and its editor
    /// both keep `.qoder`); none: its home being there is enough.
    pub marks: &'static [&'static str],
    /// How it is known to be installed, and its version read.
    pub programs: &'static [Program],
    /// Where Fastctx's MCP server is registered, and as what.
    pub mcp: Mcp,
    /// Its global instructions file, where it has one.
    pub guidance: Option<Guidance>,
    /// Turning its own shell off, each change in turn, where it can be.
    pub shell: &'static [Edit],
    /// Raising its limits on one tool's answer, where it has them.
    pub limits: &'static [Edit],
    /// Where it keeps who is logged in, where that is not its home.
    pub data: Option<Home>,
    /// Who is logged in, read from the files of its data directory, else of
    /// its home, alone: never over the network, never by starting its
    /// program.
    pub identity: fn(&Path) -> Option<Identity>,
}

/// Where an agent's user-level configuration directory is: the first of
/// `env` that is set, joined with its subdirectory, else `default` under the
/// user's home.
pub struct Home {
    /// An environment variable and the directory under it, such as
    /// (`XDG_CONFIG_HOME`, `opencode`); an empty subdirectory is the variable's
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

/// A settings file of an agent's and the key path a change is made at.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    /// Relative to the agent's home; one starting `../` is beside it (ZCode
    /// reads the `~/.agents` other agents share).
    pub file: &'static str,
    pub format: Format,
    /// The keys from the top of the file, such as `["mcpServers"]`; none is
    /// the top itself.
    pub at: &'static [&'static str],
}

/// Where a change is made: one place, or the one of several the agent reads.
#[derive(Clone, Copy, Debug)]
pub enum To {
    One(Place),
    Among(&'static [Place], Pick),
}

impl To {
    /// Every place it may be made in.
    pub fn places(&self) -> &[Place] {
        match self {
            Self::One(place) => std::slice::from_ref(place),
            Self::Among(places, _) => places,
        }
    }
}

/// Which of several places the agent reads, and so where a change goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The first whose file is there, else the first: the agent reads that
    /// one alone (Amp's `settings.json`, else its `settings.jsonc`). A file
    /// only Fastctx made is not counted.
    Present,
    /// The first that holds a server besides Fastctx's, else the first: the
    /// agent reads that one alone (ZCode's own file while it holds any
    /// server, else the one it shares).
    Holding,
}

/// Fastctx's MCP server in an agent's configuration.
pub struct Mcp {
    pub to: To,
    pub slot: Slot,
    /// The entry that starts Fastctx's server.
    pub entry: fn(&Server<'_>) -> serde_json::Value,
}

/// How the servers are kept where they are.
#[derive(Clone, Copy, Debug)]
pub enum Slot {
    /// Each a property of an object, Fastctx's named `fastctx`.
    Named,
    /// Each an item of a list, Fastctx's the one whose value at `key` is
    /// `value` (Continue's `name: fastctx`; DeepSeek's insert row).
    Listed { key: &'static [&'static str], value: &'static str },
}

/// How Fastctx's server is started: its program's absolute path, its
/// arguments, its environment.
pub struct Server<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub env: &'a [(String, String)],
}

/// An agent's global instructions file.
#[derive(Clone, Copy, Debug)]
pub struct Guidance {
    /// Relative to the agent's home.
    pub file: &'static str,
    /// What a file Fastctx makes for it starts with, where the agent reads
    /// such a file only under a header (a Cursor rule always applied).
    pub head: Option<&'static str>,
    /// The most bytes the agent reads of the file: Fastctx's passage is left
    /// out rather than written past it, to be cut off by the agent.
    pub most: Option<usize>,
}

/// The formats of configuration Fastctx edits in place, keeping the rest of
/// the file as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    /// JSON with comments and trailing commas.
    Jsonc,
    Yaml,
}

/// One change to an agent's configuration besides its server's entry; the
/// last key of a place is the setting's own.
pub enum Edit {
    /// The value set; taken out, what was there before is put back, or the
    /// key removed where there was none.
    Set { to: To, value: fn() -> serde_json::Value },
    /// A number set to at least `value`, where a negative number is no
    /// limit: one already as high, or unlimited, is left as it is. Taken
    /// out as `Set` is.
    Raise { to: To, value: fn() -> serde_json::Value },
    /// The value set where there is none: one already there is the user's,
    /// left as it is when connecting and when taking out.
    Default { to: To, value: fn() -> serde_json::Value },
    /// An item added to the list at the place; taken out, that item goes.
    Add { to: To, item: fn() -> serde_json::Value },
    /// A whole file of Fastctx's own, made on connecting and deleted on
    /// taking out, such as a policy file among the agent's.
    Own { file: &'static str, body: &'static str },
}

impl Edit {
    /// The files it may change, relative to the agent's home.
    pub fn files(&self) -> Vec<&'static str> {
        match self {
            Self::Set { to, .. } | Self::Raise { to, .. } | Self::Default { to, .. } | Self::Add { to, .. } => {
                to.places().iter().map(|place| place.file).collect()
            }
            Self::Own { file, .. } => vec![file],
        }
    }
}

impl Agent {
    /// Every file connecting it reads or writes, relative to its home, each
    /// once, in the order the planner takes them.
    pub fn files(&self) -> Vec<&'static str> {
        let mut files = Vec::new();
        let mut add = |file: &'static str| {
            if !files.contains(&file) {
                files.push(file);
            }
        };
        self.mcp.to.places().iter().for_each(|place| add(place.file));
        for edit in self.shell.iter().chain(self.limits) {
            edit.files().into_iter().for_each(&mut add);
        }
        if let Some(guidance) = self.guidance {
            add(guidance.file);
        }
        files
    }
}

/// `file` of an agent whose home is `home`: under it, or beside it where it
/// starts `../`, taken lexically so a verbatim Windows path stays one.
pub fn located(home: &Path, file: &str) -> PathBuf {
    let mut base = home.to_path_buf();
    let mut rest = file;
    while let Some(after) = rest.strip_prefix("../") {
        base.pop();
        rest = after;
    }
    base.join(rest)
}
