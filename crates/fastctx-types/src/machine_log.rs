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
    /// The account's own configuration was written; what it says is not
    /// kept here.
    Profile {
        version: u64,
    },
    Share {
        machine: MachineName,
        to: AccountName,
        shared: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        jobs: Option<u16>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daily: Option<crate::usage::Usd>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        five_hour: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turns: Option<u16>,
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
    /// A machine relays to the hub for its account's machines on its local
    /// network at `addresses`, or, with none, no longer. Before direct
    /// connections it was the relay any machine connected through.
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
        /// What it was told to do then.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        then: Option<crate::uninstall::Then>,
    },
    /// `name`, taken off and told to uninstall Fastctx, could not do all of
    /// it: what was left, and why, as it said; `by` took it off, and it was
    /// `home`'s.
    UninstallLeft {
        name: MachineName,
        left: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<AccountName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<AccountName>,
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
        /// Whether managed machines update by themselves; none before
        /// 1.0.1 beta 3, which always did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        managed_auto: Option<bool>,
        /// How fast the hub takes packages in at most, in bytes a second,
        /// when that is what was set; the rest is the policy as it stood.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intake: Option<u64>,
    },
    /// `machine`, away, is to update to `version` once it is back; none,
    /// what was kept for it was taken back.
    UpdateDue {
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// `machine` says it was updated from `from` to `to` as `by` asked.
    Updated {
        machine: MachineName,
        from: String,
        to: String,
        by: AccountName,
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
    /// Its owner deleted a project and its ledger.
    DeleteProject {
        town: crate::access::TownId,
        name: Option<TownName>,
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
    /// Choosing a machine as the account's sentinel home allows it to act for the account.
    AllowSentinelHome {
        machine: MachineName,
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

    /// Whether the record is of a project: read only by the accounts the
    /// project is theirs or shared with, the super administrator not among
    /// them unless it is (`world-network.md`, 项目也照同一个规矩).
    pub fn of_project(&self) -> bool {
        matches!(
            self,
            Self::OpenProject { .. } | Self::ShareProject { .. } | Self::MoveProject { .. } | Self::DeleteProject { .. }
        )
    }

    /// What a reader must say it reads to be given a record of this kind: a
    /// release reads a page whole, and one kind it does not know fails all
    /// of it (`world-versions.md`). A kind added later names here what its
    /// readers say, before any record of it is kept.
    pub fn needs(&self) -> Needs {
        match self {
            Self::Rename { .. } => Needs::Renames,
            Self::Forget { .. } | Self::Forgotten { .. } => Needs::Forgets,
            Self::Updates { .. } => Needs::Versions,
            Self::UpdateDue { .. } | Self::Updated { .. } | Self::UninstallLeft { .. } => Needs::Due,
            Self::DeleteProject { .. } => Needs::ProjectDeletes,
            Self::Profile { .. } => Needs::Profiles,
            Self::Collab { .. }
            | Self::Share { .. }
            | Self::Assist { .. }
            | Self::AssistEnded { .. }
            | Self::Plan { .. }
            | Self::StopActing { .. }
            | Self::Relays { .. }
            | Self::RelayAt { .. }
            | Self::Direct { .. }
            | Self::Remove { .. }
            | Self::Logout
            | Self::Token { .. }
            | Self::RevokeToken { .. }
            | Self::PasswordStart { .. }
            | Self::PasswordFinish { .. }
            | Self::OpenAccount { .. }
            | Self::SetRole { .. }
            | Self::AcceptRole { .. }
            | Self::SetAccount { .. }
            | Self::Transfer { .. }
            | Self::DeleteAccount { .. }
            | Self::Settings { .. }
            | Self::Dissolve { .. }
            | Self::OpenProject { .. }
            | Self::ShareProject { .. }
            | Self::MoveProject { .. }
            | Self::Joined { .. }
            | Self::AllowActing { .. }
            | Self::AllowSentinelHome { .. }
            | Self::ActingStopped { .. }
            | Self::Group { .. }
            | Self::RenameGroup { .. }
            | Self::Left { .. } => Needs::Nothing,
        }
    }
}

/// What reading a kind of record takes (`Action::needs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Every release reads it.
    Nothing,
    /// A reader that takes new names the hub gives.
    Renames,
    /// A reader that deletes what it keeps of a conversation when told.
    Forgets,
    /// A reader of the network's versions.
    Versions,
    /// A reader of the records of updates due and made as asked, which came
    /// in 1.0.1-beta.3 with the other kinds that release keeps.
    Due,
    /// A reader of the records of projects deleted, which came in
    /// 1.0.1-beta.6.
    ProjectDeletes,
    /// A reader of the records of profiles written, which came in
    /// 1.0.1-beta.7.
    Profiles,
}

/// What one reader of the machine log says it reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    pub renames: bool,
    pub forgets: bool,
    pub versions: bool,
    pub due: bool,
    pub project_deletes: bool,
    pub profiles: bool,
}

impl Reads {
    /// Whether this reader reads a record that `needs` this.
    pub fn covers(self, needs: Needs) -> bool {
        match needs {
            Needs::Nothing => true,
            Needs::Renames => self.renames,
            Needs::Forgets => self.forgets,
            Needs::Versions => self.versions,
            Needs::Due => self.due,
            Needs::ProjectDeletes => self.project_deletes,
            Needs::Profiles => self.profiles,
        }
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
