//! Accounts on the hubs this computer uses, as the command line and the
//! interfaces see and change them (`world-network.md`): logging in and out,
//! machines shared and agents' logins assisting other accounts, the machines
//! that act for an account, devices, tokens and passwords, and the super
//! administrator's accounts.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::access::HubId;
use crate::world::LinkState;
use crate::{AccountName, AgentName, HubName, MachineName, TownName};

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

/// What an account is on its hub (`world-network.md`, 账号只有两级).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The hub's first account, and its controller: it may do everything,
    /// on every machine. A hub has one, who may hand it to another account.
    Super,
    /// Registers its own computers as machines, shares them, lets their
    /// agents' logins assist other accounts, and opens projects.
    #[default]
    Member,
}

/// A relay a machine connected through before direct connections replaced
/// relays: a port forwarded to the hub. Only change records made then
/// still name one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relay {
    /// What the user called it, such as the machine it runs on.
    pub label: String,
    pub addresses: Vec<SocketAddr>,
}

/// What the network's administrator chooses a machine's direct address
/// from (`world-network.md`, 直连): the addresses of its own network cards,
/// and the one the hub sees its connection come from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    /// The address the hub sees it at; none while it reaches the hub
    /// through another machine, whose address that would be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<IpAddr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<IpAddr>,
}

/// How a machine's lines go, as it last told the hub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paths {
    /// The round trip of its line to the hub, in milliseconds, as last
    /// measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_ms: Option<u32>,
    /// Its direct links, and those that fell back to the hub.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkSeen>,
}

/// A machine's direct link with another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSeen {
    pub peer: MachineName,
    /// Whether it is up; when not, what goes between the two goes through
    /// the hub.
    pub up: bool,
    /// While it is up, its round trip in milliseconds as last measured;
    /// `None` before the first one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
    /// When it came up or went down.
    pub since: Timestamp,
    /// Why it is down, or why the last try to bring it up failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A change made as an account. The same as the hub takes them, except
/// that a password is given here as the user typed it; the node sets it
/// with OPAQUE and it never leaves this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "change")]
pub enum AccountChange {
    /// Shares `machine`, one of the account's own, with `to`: working on it
    /// and every agent logged in on it; or no longer.
    Share {
        machine: MachineName,
        to: AccountName,
        shared: bool,
    },
    /// Lets one login of an agent on `machine`, one of the account's own,
    /// assist `to`; or no longer. `home` is the login's directory as
    /// `machine` writes it, none for that agent's default login there.
    /// `until` is when the assistance ends by itself; assisting again sets
    /// it anew.
    Assist {
        machine: MachineName,
        to: AccountName,
        agent: AgentName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<PathBuf>,
        assisting: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<Timestamp>,
    },
    /// The plan shown for one login of an agent on `machine`, set by the
    /// hub's super administrator on a login that names none (a Claude Code
    /// long-lived token); none takes it away. Shown, never acted on.
    Plan {
        machine: MachineName,
        agent: AgentName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<PathBuf>,
        plan: Option<PlanLabel>,
    },
    /// `machine`, another's, no longer acts for the account.
    StopActing {
        machine: MachineName,
    },
    /// Allows direct connections to `machine` at `at`, the super
    /// administrator's change: the hub tries them from outside first, and
    /// they count only when that works. None, no longer.
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
    },
    SetPassword {
        target: AccountName,
        password: String,
    },
    OpenAccount {
        name: AccountName,
        password: String,
    },
    SetAccount {
        name: AccountName,
        locked: bool,
    },
    /// Hands being the hub's super administrator to `to`; the account that
    /// hands it on becomes a member.
    Transfer {
        to: AccountName,
    },
    DeleteAccount {
        name: AccountName,
    },
    Settings {
        self_registration: bool,
    },
    ShareProject {
        town: TownName,
        with: AccountName,
        shared: bool,
    },
    MoveProject {
        town: TownName,
        to: AccountName,
    },
    /// The user saw that `name` came into the account; nothing goes to the
    /// hub.
    Seen {
        name: MachineName,
    },
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
    /// The hub's own addresses, as it publishes them: what another device
    /// is given to reach it.
    #[serde(default)]
    pub addresses: Vec<SocketAddr>,
    pub link: LinkState,
    /// The accounts this computer is logged in to there.
    pub logins: Vec<AccountName>,
    /// The account this computer is registered under there, as a machine.
    pub home: Option<AccountName>,
    /// Whether anyone who reaches the hub may open an account.
    pub self_registration: bool,
    pub accounts: Vec<AccountSeen>,
    pub members: Vec<MemberSeen>,
    pub shares: Vec<ShareSeen>,
    pub assists: Vec<AssistSeen>,
    /// The plans the hub's super administrator set on logins that name
    /// none.
    #[serde(default)]
    pub plans: Vec<PlanSeen>,
    /// Machines of others that act for an account, because it used an
    /// agent there (`world-network.md`, 别人机器上的智能体什么时候能访问你的机器).
    pub acting: Vec<ActingSeen>,
    pub projects: Vec<ProjectSeen>,
    /// Who came into the accounts this computer is logged in to there,
    /// not yet shown to the user.
    #[serde(default)]
    pub arrivals: Vec<Arrival>,
    /// This computer's direct links with the machines there, as it holds
    /// them now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkSeen>,
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
    pub level: Level,
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
    /// Where other machines connect to it directly, when the network's
    /// administrator allowed that.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<SocketAddr>,
    pub added: Timestamp,
    pub online: bool,
    /// When it connected, when online; when it was last connected, when not.
    pub since: Option<Timestamp>,
    /// The machine it reaches the hub through, when its own line is down.
    pub through: Option<String>,
    /// What its direct address may be chosen from, for the super
    /// administrator, while it is online.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    /// How its lines go, as it last said, while it is online.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Paths>,
    /// The start of its key, to tell two devices of one name apart.
    pub key: String,
}

/// A machine shared with an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareSeen {
    pub machine: MachineName,
    pub account: AccountName,
}

/// One login of an agent on a machine, assisting an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistSeen {
    pub machine: MachineName,
    pub account: AccountName,
    pub agent: AgentName,
    /// The login's directory as that machine writes it; none for the
    /// agent's default login there.
    pub home: Option<PathBuf>,
    /// When the assistance ends by itself; none while the owner keeps it
    /// on.
    pub until: Option<Timestamp>,
}

/// A plan a login names none of, as the hub's super administrator set it
/// by hand (`world-agent-logins.md`): Claude Code's long-lived tokens say no
/// plan. It is only shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanLabel {
    Max,
    Pro,
    Free,
}

/// The plan set on one login of an agent on a machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSeen {
    pub machine: MachineName,
    pub agent: AgentName,
    /// The login's directory as that machine writes it; none for the
    /// agent's default login there.
    pub home: Option<PathBuf>,
    pub plan: PlanLabel,
}

/// A machine of another that acts for an account, since it first used an
/// agent there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActingSeen {
    pub machine: MachineName,
    pub account: AccountName,
    pub since: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSeen {
    pub name: TownName,
    pub owner: AccountName,
    pub granted: Vec<AccountName>,
}
