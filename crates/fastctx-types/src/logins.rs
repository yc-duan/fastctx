//! What each machine's node keeps of its agents' logins
//! (`world-agent-logins.md`): what the last scan found there, which logins
//! the World lists and uses, which are only candidates, and how far a scan
//! or a change has got. Every page reads this record; reading it starts no
//! process. A scan happens only when the user asks for one.
//!
//! A login is a directory: Codex's `CODEX_HOME`, Claude Code's
//! `CLAUDE_CONFIG_DIR`. The World lists exactly the logins its tools are put
//! into (`fastctx apply`); putting them in lists a candidate, taking them
//! out (removing it) returns it to the candidates.

use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agents::{AgentLimits, AgentModel, Fact, Integration};
use crate::{AgentName, MachineName};

/// The record of every machine asked: this one, or each the asker may use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldLogins {
    pub machine: MachineName,
    pub machines: Vec<MachineLoginsHeard>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MachineLoginsHeard {
    Answered { logins: Box<MachineLogins> },
    Offline { machine: MachineName, since: Option<Timestamp> },
    NoAnswer { machine: MachineName, reason: String },
}

/// One machine's record of its agents' logins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineLogins {
    pub machine: MachineName,
    /// The scan running now, or the last one; none before the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,
    pub codex: AgentLogins,
    pub claude: AgentLogins,
}

impl MachineLogins {
    pub fn of(&self, agent: &str) -> Option<&AgentLogins> {
        match agent {
            "codex" => Some(&self.codex),
            "claude" => Some(&self.claude),
            _ => None,
        }
    }

    pub fn of_mut(&mut self, agent: &str) -> Option<&mut AgentLogins> {
        match agent {
            "codex" => Some(&mut self.codex),
            "claude" => Some(&mut self.claude),
            _ => None,
        }
    }
}

/// One agent on a machine: its programs, and its logins in three groups.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLogins {
    /// Its programs installed here, as their manifests and version
    /// information say, none of them run; empty before the first scan.
    pub programs: Vec<FoundProgram>,
    /// The logins the World lists and uses: those its tools are put into.
    pub listed: Vec<Login>,
    /// The other logins a scan found in the usual places, or the user
    /// added: each can be put into the list.
    pub candidates: Vec<Login>,
    /// Directories the walk of every disk found that look like logins of
    /// this agent; no one has asked who is logged in there.
    pub elsewhere: Vec<LooksLike>,
}

/// One login: a directory of an agent's own, and what is known of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    /// As this machine writes it.
    pub dir: PathBuf,
    /// The one the agent's programs use when nothing names another: from
    /// `CODEX_HOME` or `CLAUDE_CONFIG_DIR` in the user's environment, else
    /// `~/.codex`, `~/.claude`.
    pub default: bool,
    pub found: Found,
    /// Who is logged in, as the agent itself said when it was last asked;
    /// none before it was asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identified>,
    /// Its usage windows as last read on this machine, with when; never
    /// read is unavailable, never 0% used.
    pub limits: Fact<AgentLimits>,
    /// The programs of this agent here that use this login (indexes into
    /// `AgentLogins::programs`): putting the World's tools into the login
    /// gives every one of them the World's `run`.
    pub programs: Vec<u32>,
    /// Whether the World's tools are in: `In` for every listed login; a
    /// candidate is `Out`, or `Old` where an old fastctx put its own in.
    pub integration: Integration,
    /// For Claude Code: whether the World's conversations start in it
    /// without asking for approvals (its `skipDangerousModePermissionPrompt`),
    /// which putting the World's tools in grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unattended: Option<bool>,
    /// The program the World starts for this login, when one was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    /// The models a conversation in it can start on, as last read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AgentModel>,
    /// The change running in it now, or the last one since the node
    /// started, step by step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<LoginChange>,
}

/// How the World knows of a login.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Found {
    /// The agent's default login.
    Default,
    /// A directory directly in the user's home, named and kept like the
    /// agent's.
    Home,
    /// One the user wrote in.
    Added,
    /// One the walk of every disk found, which the user then took up.
    Elsewhere,
}

/// What the agent said when asked who is logged in, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identified {
    pub at: Timestamp,
    pub who: Fact<Identity>,
}

/// Who is logged in to a login, as its agent says without anyone reading
/// its credentials.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub logged_in: bool,
    /// How, as the agent names it: Codex `chatgpt` or `apikey`; Claude
    /// Code `claude.ai` (a `/login` subscription login) or `oauth_token` (a
    /// long-lived token from the environment, such as `setup-token`'s, which
    /// names no account and no plan), and others as it names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// The plan, as the agent names it: `plus`, `pro`, `max`, ….
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// The program the World starts for a login.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launcher {
    pub path: PathBuf,
    /// Its version, as it said itself when last started (Codex), or as the
    /// program's own records say (Claude Code); none when neither did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether it is the program a desktop app keeps for itself.
    pub desktop_app: bool,
}

/// A program of an agent installed on a machine, as found without running
/// it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundProgram {
    /// The command-line program itself; none where what it comes inside
    /// holds none yet (a desktop app that downloads it when first used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub how: Installed,
    /// The command-line program's own version, as its package manifest or
    /// its file's version information says; none when neither says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The editor extension or desktop app it comes inside, with that one's
    /// own version, which is not the program's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<Within>,
    /// Whether typing the agent's command in a terminal here runs it.
    pub on_path: bool,
}

/// How a program came to be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Installed {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    /// The agent's own installer.
    Official,
    Winget,
    Scoop,
    Chocolatey,
    Homebrew,
    /// Inside a desktop app.
    DesktopApp,
    /// Inside an editor's extension.
    Extension,
    /// Found on the search path, by no manager the World knows.
    Path,
}

/// What a program comes inside.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Within {
    /// The editor or app, as people call it: `VS Code`, `Cursor`,
    /// `ChatGPT`, `Claude`.
    pub name: String,
    /// The extension's or package's own identifier: `openai.chatgpt`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Where the extension or app is.
    pub path: PathBuf,
}

/// A directory that looks like a login, and the names in it that make it
/// look so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LooksLike {
    pub dir: PathBuf,
    pub traces: Vec<String>,
}

/// A scan of a machine for its agents: first the usual places, then, at
/// the lowest priority, a walk of every local fixed disk by names alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scan {
    pub started_at: Timestamp,
    /// The agents' default logins, the first level of the user's home and
    /// the directories the user added, each asked who is logged in there;
    /// and the agents' programs in the places they are installed.
    pub usual: Stage,
    /// The walk of every disk; none once the usual places were not done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disks: Option<DiskWalk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Stage {
    Running,
    Done { at: Timestamp },
    /// The user stopped it.
    Stopped { at: Timestamp },
    Failed { at: Timestamp, reason: String },
}

/// How far the walk of every disk has got.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskWalk {
    pub stage: Stage,
    pub disks: Vec<DiskProgress>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskProgress {
    pub root: PathBuf,
    /// Directories listed so far.
    pub directories: u64,
    /// Directories that could not be listed, such as another user's.
    pub unreadable: u64,
    pub done: bool,
}

/// Putting the World's tools into a login, or taking them out, as the node
/// of its machine does it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginChange {
    /// Putting in (apply), or taking out (unapply, which removes the login
    /// from the list).
    pub into: bool,
    pub started_at: Timestamp,
    pub steps: Vec<ChangeStep>,
    /// How it ended; none while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<ChangeEnd>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeStep {
    pub step: Step,
    pub state: StepState,
}

/// One step of a change, in the order they run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum Step {
    /// The World's program put where the agents start it from.
    Program,
    /// One of Claude Code's own commands, as it runs.
    Command { command: String },
    /// Every file of the change written at once, with the record of what
    /// was written; the files once they are.
    Files {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changed: Vec<PathBuf>,
    },
    /// The agent's own tools that the World's replace checked to be off.
    Check { scope: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum StepState {
    Running,
    Done,
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "end")]
pub enum ChangeEnd {
    Done {
        at: Timestamp,
        /// What stayed as the user had it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        kept: Vec<Kept>,
    },
    Failed {
        at: Timestamp,
        reason: String,
        /// The change needs Codex's shared output limit raised, which only
        /// the user's explicit word does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_limit: Option<OutputLimit>,
    },
}

/// Something a change leaves as the user has it, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub location: String,
    pub reason: String,
}

/// Codex's `tool_output_token_limit`, which every tool shares, set lower by
/// the user than the World's answers need.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputLimit {
    pub location: String,
    pub current: i64,
    pub requested: u64,
}

/// What a change would do, worked out from the files as they are now, with
/// nothing written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePreview {
    pub agent: AgentName,
    pub dir: PathBuf,
    pub into: bool,
    /// Every file it would write, and each item in it that would change.
    pub files: Vec<FilePreview>,
    /// Claude Code's own commands it would run, which write Claude Code's
    /// plugin settings themselves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Set when putting the tools in needs Codex's shared output limit
    /// raised: the change then raises it only when the user says so, and
    /// `files` shows it raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_limit: Option<OutputLimit>,
    /// What it would leave as the user has it: for taking out, what the
    /// user changed since the tools were put in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Kept>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePreview {
    pub path: PathBuf,
    pub how: FileHow,
    pub items: Vec<ItemChange>,
}

/// Whether a file or an item would be created, changed or removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileHow {
    Created,
    Changed,
    Removed,
}

/// One item of a file that would change: a setting by its key path
/// (`features.shell_tool`, `env.MAX_MCP_OUTPUT_TOKENS`), or the World's
/// section of a guidance file. Values are shown only for what the World
/// itself writes there; a value that might be the user's secret is withheld
/// and only its key is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemChange {
    pub item: String,
    pub how: FileHow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withheld: bool,
}
