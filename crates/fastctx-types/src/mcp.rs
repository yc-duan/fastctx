//! What a device or a session asks its node of an account's MCP servers
//! (`world-user-config.md`): the account's list, with what each server's
//! machine says of it; keeping, removing and trying one; and what the
//! tools on a machine already have configured, to take from.

use serde::{Deserialize, Serialize};

use crate::MachineName;
use crate::access::Written;
use crate::profile::{CommandMcp, RemoteMcp, Server};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum McpCall {
    /// The account's servers, each with what its machine says of it.
    List,
    /// Adds the server `name`, or replaces the one of that name: a remote
    /// one on the account; a command one on `machine`, or where none is
    /// named, on the account's one machine, or on the machine the asking
    /// session runs on when it is the account's own.
    Keep {
        name: String,
        server: Server,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<Written>,
    },
    /// Imports a candidate on its machine without returning its command or environment.
    Import {
        machine: Written,
        source: Source,
        name: String,
        as_name: String,
    },
    Remove {
        name: String,
    },
    /// Starts it once as a session would, and lists its tools.
    Test {
        name: String,
    },
    /// What Claude Desktop, Claude Code and Cursor on `machine` have
    /// configured; on the machine of the device or session asking when
    /// none is named.
    Found {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<Written>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum McpReply {
    Listed {
        servers: Vec<Listed>,
    },
    /// The account's profile version after the change.
    Kept {
        name: String,
        version: u64,
    },
    Removed {
        version: u64,
    },
    Tested(Tested),
    Found {
        machine: MachineName,
        servers: Vec<Found>,
    },
}

/// One of the account's servers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub name: String,
    pub server: ListedServer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ListedServer {
    Remote(RemoteMcp),
    Command {
        machine: MachineName,
        state: CommandState,
    },
    /// Of a kind this release does not know.
    Unknown,
}

/// What the machine of a command server says of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum CommandState {
    /// The machine keeps it so.
    Kept,
    /// The machine is not connected now.
    Offline,
    /// The machine keeps no server of that name for the account.
    Missing,
    /// The machine runs a release that keeps no MCP servers.
    Older,
    /// Asking the machine failed.
    Failed { problem: String },
}

/// How starting a server once went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tested {
    /// Its tools' names, when it started and listed them.
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// The end of what it wrote to its standard error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

/// A server one of the tools on a machine has configured.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    pub source: Source,
    pub name: String,
    pub server: FoundServer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "app")]
pub enum Source {
    ClaudeDesktop,
    /// A Claude Code login, by its directory there.
    ClaudeCode {
        home: String,
    },
    Cursor,
}

/// The account's profile as its device reads it to change it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileView {
    pub hub: crate::HubName,
    pub account: crate::AccountName,
    pub profile: crate::profile::Profile,
    /// Whether the hub keeps profiles at all; an older one does not, and
    /// takes no change to one.
    pub kept: bool,
}

/// Imported command contents remain on the machine that found them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FoundServer {
    Command,
    Remote(RemoteMcp),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum MachineCall {
    Status { names: Vec<String> },
    Keep { name: String, version: u64, server: CommandMcp },
    Import { name: String, version: u64, source: Source, original: String },
    Found,
    Test { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reply")]
pub enum MachineReply {
    Status { kept: std::collections::BTreeSet<String> },
    Kept,
    Found { servers: Vec<Found> },
    Tested(Tested),
}
