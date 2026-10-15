//! The hub's machine-layer changes, without login or password material.

use std::net::SocketAddr;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::accounts::{MemberKind, Relay};
use crate::{AccountName, AgentName, MachineName, TownName};

/// The device the hub recognized and the account the request named.
/// An unverified account claim is marked explicitly. Account opening and
/// local operations may have no registered caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub account: Option<AccountName>,
    /// Whether the hub verified the account relationship needed by this
    /// request. False marks a rejected claim, rather than that person's action.
    #[serde(default)]
    pub authenticated: bool,
    pub member: Option<MachineName>,
    pub peer: Option<SocketAddr>,
}

/// An account change's public facts. Password exchanges, token secrets,
/// and collaboration prompts are deliberately absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum Action {
    Collab {
        version: u64,
    },
    Share {
        machine: MachineName,
        to: AccountName,
        shared: bool,
    },
    /// Written under the name it was first written under, so hubs and
    /// nodes of every version read one another's records.
    #[serde(rename = "lend")]
    Assist {
        machine: MachineName,
        to: AccountName,
        agent: AgentName,
        home: Option<PathBuf>,
        #[serde(rename = "lent")]
        assisting: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<Timestamp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekly: Option<crate::usage::Usd>,
    },
    /// An assistance reached its deadline and the hub ended it.
    #[serde(rename = "lend_ended")]
    AssistEnded {
        machine: MachineName,
        to: AccountName,
        agent: AgentName,
        home: Option<PathBuf>,
    },
    Plan {
        machine: MachineName,
        agent: AgentName,
        home: Option<PathBuf>,
        plan: Option<crate::accounts::PlanLabel>,
    },
    StopActing {
        machine: MachineName,
    },
    /// Before direct connections replaced relays: the relays a machine
    /// connected through.
    Relays {
        machine: MachineName,
        relays: Vec<Relay>,
    },
    /// Before direct connections replaced relays: a machine made a relay at
    /// `addresses`, or, with none, no longer.
    RelayAt {
        machine: MachineName,
        addresses: Vec<SocketAddr>,
    },
    /// Direct connections to `machine` allowed at `at`, or, with none, no
    /// longer.
    Direct {
        machine: MachineName,
        at: Vec<SocketAddr>,
    },
    Remove {
        name: MachineName,
        reason: String,
    },
    Logout,
    Token {
        register: bool,
        name: Option<MachineName>,
        kind: MemberKind,
        expires: Timestamp,
        /// For managed machines (`world-network.md`, 受管机器).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        managed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// How many devices it lets in, one when none, any number at zero.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uses: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        way: Option<crate::accounts::TokenWay>,
    },
    RevokeToken {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        way: Option<crate::accounts::TokenWay>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        managed: bool,
    },
    PasswordStart {
        target: AccountName,
    },
    PasswordFinish {
        target: AccountName,
    },
    OpenAccount {
        name: AccountName,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        visitor: bool,
    },
    SetRole {
        name: AccountName,
        visitor: bool,
    },
    AcceptRole {
        accept: bool,
    },
    SetAccount {
        name: AccountName,
        locked: bool,
    },
    Transfer {
        to: AccountName,
    },
    DeleteAccount {
        name: AccountName,
    },
    Settings {
        self_registration: bool,
    },
    /// The network's update policy was set (`crate::update::Policy`).
    Updates {
        channel: crate::update::Channel,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minimum: Option<String>,
    },
    /// The network named `name` was dissolved.
    Dissolve {
        name: crate::HubName,
    },
    OpenProject {
        town: crate::access::TownId,
        name: TownName,
    },
    ShareProject {
        town: crate::access::TownId,
        /// Its short name at the time of the attempt, when known to the hub.
        name: Option<TownName>,
        with: AccountName,
        shared: bool,
    },
    MoveProject {
        town: crate::access::TownId,
        /// Kept even when an archived project is handed to another account.
        name: Option<TownName>,
        to: AccountName,
    },
    /// A password login or token use. `registered` says whether the device
    /// is a machine of the joining account after a successful login.
    Joined {
        name: MachineName,
        kind: MemberKind,
        registered: bool,
        via: LoginMethod,
        /// As a managed machine: registered, logged in to none.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        managed: bool,
    },
    /// First use of another machine's agent allows it to act for the account.
    AllowActing {
        machine: MachineName,
        agent: AgentName,
        home: Option<PathBuf>,
    },
    /// A permission removed implicitly by a share, an assistance, or an
    /// account change.
    ActingStopped {
        machine: MachineName,
        account: AccountName,
    },
    /// A machine renamed; its names before still name it.
    Rename {
        machine: MachineName,
        to: MachineName,
    },
    /// A machine put in a group, or in none.
    Group {
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// A group of an account's machines renamed, or ended.
    RenameGroup {
        owner: AccountName,
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// A membership removed by logout, removal, or account deletion.
    Left {
        name: MachineName,
        account: AccountName,
        kind: MemberKind,
        unregistered: bool,
        reason: String,
    },
    /// The user asked that a conversation be deleted from every machine and
    /// device keeping something of it.
    Forget {
        conversation: crate::Address,
    },
    /// What `on` kept of a conversation being deleted is gone; refused, it
    /// could not be deleted there yet, and is asked again when `on` comes
    /// back.
    Forgotten {
        conversation: crate::Address,
        on: MachineName,
    },
}

impl Action {
    /// Whether the record is of a conversation: read only by the account
    /// it worked for, never by another, the super administrator included
    /// (`world-gui.md`, 界面的结构).
    pub fn of_conversation(&self) -> bool {
        matches!(self, Self::Forget { .. } | Self::Forgotten { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginMethod {
    Password,
    Token,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Outcome {
    Done,
    Refused { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Monotonically increasing on this hub; also the pagination cursor.
    pub id: u64,
    pub at: Timestamp,
    pub actor: Actor,
    pub action: Action,
    pub outcome: Outcome,
}

/// Newest first. Pass `next_before` as `before` to read the next page.
/// None means no older visible record remains.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub records: Vec<Record>,
    pub next_before: Option<u64>,
}
