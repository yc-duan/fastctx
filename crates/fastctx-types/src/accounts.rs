//! Accounts on the hubs this computer uses, as the command line and the
//! interfaces see and change them (`world-network.md`): logging in and out,
//! who may use which machine, which machines an account trusts, devices,
//! tokens and passwords, and an administrator's accounts.

use std::net::SocketAddr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::access::{HubId, Rights};
use crate::world::LinkState;
use crate::{AccountName, HubName, MachineName, TownName};

/// What a member of a hub is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    /// A computer: a node runs on it, and it may be registered as a machine.
    #[default]
    Computer,
    /// A phone: it carries its user's words and looks; nothing runs on it.
    Phone,
    /// A web AI's connection through the hub's endpoint.
    Web,
}

impl MemberKind {
    pub fn is_computer(self) -> bool {
        self == MemberKind::Computer
    }
}

/// A relay a machine connects through: a port forwarded to the hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    /// What the user called it, such as the machine it runs on.
    pub label: String,
    pub addresses: Vec<SocketAddr>,
}

/// A change made as an account. The same as the hub takes them, except
/// that a password is given here as the user typed it; the node sets it
/// with OPAQUE and it never leaves this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum AccountChange {
    Grant { machine: MachineName, to: AccountName, rights: Rights },
    Trust { machine: MachineName, trusted: bool },
    Relays { machine: MachineName, relays: Vec<Relay> },
    Remove { name: MachineName, reason: String },
    Logout,
    Token { register: bool, name: Option<MachineName>, kind: MemberKind, expires: Timestamp },
    SetPassword { target: AccountName, password: String },
    OpenAccount { name: AccountName, admin: bool, may_register: bool, password: String },
    SetAccount { name: AccountName, admin: bool, may_register: bool, locked: bool },
    DeleteAccount { name: AccountName },
    Settings { self_registration: bool },
    ShareProject { town: TownName, with: AccountName, shared: bool },
    MoveProject { town: TownName, to: AccountName },
    /// The user saw that `name` came into the account; nothing goes to the
    /// hub.
    Seen { name: MachineName },
}

/// Every hub this computer is on, as the user sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountsReply {
    /// This computer's name, the same on every hub.
    pub machine: MachineName,
    pub hubs: Vec<HubAccounts>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubAccounts {
    pub hub: HubId,
    pub name: HubName,
    pub link: LinkState,
    /// The accounts this computer is logged in to there.
    pub logins: Vec<AccountName>,
    /// The account this computer is registered under there, as a machine.
    pub home: Option<AccountName>,
    /// Whether anyone who reaches the hub may open an account.
    pub self_registration: bool,
    pub accounts: Vec<AccountSeen>,
    pub members: Vec<MemberSeen>,
    pub grants: Vec<GrantSeen>,
    pub trust: Vec<TrustSeen>,
    pub projects: Vec<ProjectSeen>,
    /// Who came into the accounts this computer is logged in to there,
    /// not yet shown to the user.
    #[serde(default)]
    pub arrivals: Vec<Arrival>,
}

/// A device that logged in to an account, or a machine registered under
/// it, as one of the account's other logins noticed it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrival {
    pub hub: HubId,
    pub account: AccountName,
    pub name: MachineName,
    pub device: bool,
    pub noticed_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSeen {
    pub name: AccountName,
    pub admin: bool,
    pub may_register: bool,
    pub locked: bool,
    pub opened: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSeen {
    pub name: MachineName,
    pub kind: MemberKind,
    pub logins: Vec<AccountName>,
    /// The account it is registered under, when it is a machine.
    pub home: Option<AccountName>,
    pub relays: Vec<Relay>,
    pub added: Timestamp,
    pub online: bool,
    /// When it connected, when online; when it was last connected, when not.
    pub since: Option<Timestamp>,
    /// The relay it is connected through.
    pub through: Option<String>,
    /// The start of its key, to tell two devices of one name apart.
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantSeen {
    pub machine: MachineName,
    pub account: AccountName,
    pub rights: Rights,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustSeen {
    pub account: AccountName,
    pub machine: MachineName,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSeen {
    pub name: TownName,
    pub owner: AccountName,
    pub granted: Vec<AccountName>,
}
