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

/// A kind of device a later hub adds is read as a phone's: a device that
/// signs in, with nothing run on it.
impl crate::tolerant::Tolerant for MemberKind {
    const ALL: &'static [Self] = &[MemberKind::Computer, MemberKind::Phone, MemberKind::Web];
    const UNKNOWN: Self = MemberKind::Phone;
}

/// What an account is on its hub (`world-network.md`, 一个中枢，几个账号).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The hub's first account, and its controller: it may do everything,
    /// on every member's machine. A hub has one, who may hand it to a
    /// member.
    Super,
    /// Registers its own computers as machines, shares them, lets their
    /// agents' logins assist other accounts, and opens projects; the super
    /// administrator may use its machines.
    #[default]
    Member,
    /// Registers its own computers as machines that are its alone: nobody
    /// else may use them, the super administrator included, who only sees
    /// which there are. It shares none and lends no agent; what others
    /// share with it, and the agents assisting it, it uses. Sent only to a
    /// device that says it reads it (`Intent::Session.roles`).
    Visitor,
}

/// A level a later hub adds is read as a visitor's: it is given no more here
/// than a visitor is.
impl crate::tolerant::Tolerant for Level {
    const ALL: &'static [Self] = &[Level::Super, Level::Member, Level::Visitor];
    const UNKNOWN: Self = Level::Visitor;
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
    /// `until` is when the assistance ends by itself; `weekly` is what
    /// `to` may spend of a Claude Code login in a week of its own seven-day
    /// window, `daily` in any 24 hours; `five_hour` holds it to a share of
    /// the login's own five-hour window; assisting again sets them all anew.
    Assist {
        machine: MachineName,
        to: AccountName,
        agent: AgentName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<PathBuf>,
        assisting: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<Timestamp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weekly: Option<crate::usage::Usd>,
        /// What `to` may spend of a Claude Code login in any 24 hours.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daily: Option<crate::usage::Usd>,
        /// The share of the login's own five-hour window, in percent, past
        /// which it assists no one: the owner's own use counts too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        five_hour: Option<u8>,
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
        /// What the machine does once it hears it was taken off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        then: Option<crate::uninstall::Then>,
    },
    /// Renames `machine`, one of the account's own or, for the hub's super
    /// administrator, any; the names it had keep naming it
    /// (`world-network.md`, 机器的名字).
    Rename {
        machine: MachineName,
        to: MachineName,
    },
    /// Puts `machine`, one of the account's own or, for the hub's super
    /// administrator, a member's, in the group named `group`, or in none
    /// (`world-network.md`, 机器的分组).
    Group {
        machine: MachineName,
        group: Option<String>,
    },
    /// Renames the group `from` of `owner`'s machines to `to`, or, with
    /// none, ends it: its machines are in no group.
    RenameGroup {
        owner: AccountName,
        from: String,
        to: Option<String>,
    },
    Logout,
    /// A token for another device; `managed` makes the computers that use
    /// it managed machines of the account (`world-network.md`, 受管机器),
    /// which only a hub that says so registers. `uses` is how many
    /// devices may use it: one when none, any number at zero. `label`
    /// names a code that is kept a while, as 「机房 A」; `expires` at
    /// `LASTING`, it lasts until it is revoked. `way` is how it is given,
    /// for its maker to know it again.
    Token {
        register: bool,
        name: Option<MachineName>,
        kind: MemberKind,
        expires: Timestamp,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        managed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uses: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        way: Option<TokenWay>,
    },
    /// No device comes in with the token whose key is `token` any more;
    /// those that came in stay.
    RevokeToken {
        token: String,
    },
    SetPassword {
        target: AccountName,
        password: String,
    },
    /// Opens an account, a visitor's when `visitor`.
    OpenAccount {
        name: AccountName,
        password: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        visitor: bool,
    },
    /// The super administrator makes `name` a visitor, at once, or a
    /// member, once the account itself agrees (`AcceptRole`).
    SetRole {
        name: AccountName,
        visitor: bool,
    },
    /// The account agrees to become a member, or does not.
    AcceptRole {
        accept: bool,
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
    /// The version the network asks each machine to run at least, and
    /// whether its managed machines update by themselves (`world-upgrade.md`).
    Updates {
        policy: crate::update::Policy,
    },
    /// How fast the hub takes packages in at most, in bytes a second; none,
    /// its default (`world-network.md`, 中枢替网络存的东西).
    Intake {
        rate: Option<u64>,
    },
    /// `machine`, away from the network, updates to `version` once it is
    /// back; none takes back what was kept (`world-upgrade.md`, 别的机器的更新).
    UpdateDue {
        machine: MachineName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// Reserves the package for a batch without telling this machine to start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch: Option<String>,
    },
    /// The super administrator dissolves the network named `name`, typed as
    /// it is (`world-network.md`, 解散网络).
    Dissolve {
        name: HubName,
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
    /// The owner deletes a project and its ledger, on a hub that says it
    /// deletes projects.
    DeleteProject {
        town: TownName,
    },
    /// The owner lets one of their machines relay their others to the hub at
    /// addresses on its own network, or, with none, no longer.
    Relay {
        machine: MachineName,
        at: Vec<SocketAddr>,
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
    /// The names this computer had, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub was: Vec<MachineName>,
    pub hubs: Vec<HubAccounts>,
    /// The networks it was on that were dissolved, not yet seen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dissolved: Vec<Dissolution>,
}

/// A network this computer was on, dissolved by its administrator: the
/// computer left it then, and says so once (`world-network.md`, 解散网络).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dissolution {
    pub hub: HubName,
    pub at: Timestamp,
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
    /// This computer's name there: its name, unless that hub has not
    /// followed a rename yet. None from an older node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub me: Option<MachineName>,
    /// The current hub host, absent when unknown or outside this device's view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_machine: Option<MachineName>,
    /// Changes to held machines by other accounts, keyed by hub/account/id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machine_notices: Vec<crate::sharing::Notice>,
    /// Whether the hub renames machines.
    #[serde(default)]
    pub renames: bool,
    /// Whether the hub registers managed machines (`world-network.md`,
    /// 受管机器).
    #[serde(default)]
    pub managed: bool,
    /// Whether the hub has visitors (`world-network.md`, 一个中枢，几个账号).
    #[serde(default)]
    pub roles: bool,
    /// Whether the hub keeps the groups of machines (`world-network.md`,
    /// 机器的分组).
    #[serde(default)]
    pub groups: bool,
    /// Whether the hub dissolves its network when its super administrator
    /// asks (`world-network.md`, 解散网络).
    #[serde(default)]
    pub dissolves: bool,
    /// Whether the hub deletes a project its owner asks it to.
    #[serde(default)]
    pub deletes_projects: bool,
    /// Whether the hub keeps the machines that relay for their owner's
    /// others on their network.
    #[serde(default)]
    pub lan_relays: bool,
    /// Whether the hub keeps the network's versions and hands out packages
    /// (`world-upgrade.md`).
    #[serde(default)]
    pub versions: bool,
    /// The hub prepares packages and retains a batch's demand until withdrawn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub package_progress: bool,
    /// Whether the hub keeps updates for machines away until they are back,
    /// and tells its version (`world-upgrade.md`, 别的机器的更新).
    #[serde(default)]
    pub due: bool,
    /// Whether the hub tells a machine it takes off to uninstall Fastctx
    /// (`world-network.md`, 移出网络与卸载).
    #[serde(default)]
    pub uninstalls: bool,
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
    /// The tokens this computer's logins there made that still let devices
    /// in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<TokenSeen>,
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
    /// The token a managed machine came in by: those of one code are told
    /// together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// What a token lets a computer into, as its hub told before it is used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenPeek {
    pub hub: HubName,
    pub account: AccountName,
    /// It makes this computer a managed machine of `account`.
    pub managed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub expires: Timestamp,
    /// Why this computer may not come in by it, when it may not: a managed
    /// machine starts from a World never used, on no other network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<Unfit>,
}

/// Why a World may not become a managed machine (`world-network.md`,
/// 受管机器): it starts from one never used, on no other network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum Unfit {
    /// This computer is on another network.
    Network,
    /// Its World has conversations.
    Conversations,
    /// An agent on this computer has the World's tools in it.
    Integrated,
    /// Whether an agent here has the World's tools is not known.
    Unknown { detail: String },
}

impl std::fmt::Display for Unfit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unfit::Network => f.write_str("this computer is on another network: a managed machine is on one network only"),
            Unfit::Conversations => f.write_str(
                "this computer's Fastctx has conversations: a managed machine starts from a Fastctx never used; remove Fastctx with everything first",
            ),
            Unfit::Integrated => f.write_str(
                "agents on this computer have Fastctx's tools in them: a managed machine starts from a Fastctx never used; remove Fastctx with everything first",
            ),
            Unfit::Unknown { detail } => write!(f, "whether agents on this computer have Fastctx's tools in them is not known: {detail}"),
        }
    }
}

/// How a code reaches the devices it lets in, for whoever made it to know
/// it again: a phone scans it, a Linux host runs the line it is in, a
/// computer has it pasted, or this computer's own SSH session with a host
/// uses it at once. It is only told, never acted on; a way a later release
/// names reads as `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenWay {
    Scan,
    Line,
    Paste,
    Ssh,
    #[serde(other)]
    Other,
}

/// When a code that lasts until it is revoked is said to expire: the end
/// of the last day a timestamp holds, as a certificate with no end says
/// the last day of 9999.
pub const LASTING: Timestamp = Timestamp::constant(253_402_128_000, 0);

/// Whether a code expiring at `expires` lasts until it is revoked.
pub fn lasting(expires: Timestamp) -> bool {
    expires >= LASTING
}

/// A token that still lets devices in, as the accounts it lets into see it,
/// and the super administrator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSeen {
    /// Its key, by which it is revoked.
    pub key: String,
    pub account: AccountName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(deserialize_with = "crate::tolerant::read")]
    pub kind: MemberKind,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    /// How many devices may use it: any number at zero.
    pub uses: u32,
    /// How many have.
    pub used: u32,
    /// At `LASTING`, never.
    pub expires: Timestamp,
    /// How it is given; an earlier hub does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub way: Option<TokenWay>,
    /// The name the one device it is for takes, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<MachineName>,
    /// The device that made it, and when; an earlier hub does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by: Option<MachineName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSeen {
    pub name: AccountName,
    #[serde(deserialize_with = "crate::tolerant::read")]
    pub level: Level,
    pub locked: bool,
    pub opened: Timestamp,
    /// Made a member by the super administrator, waiting for the account
    /// to agree.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub promotion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberSeen {
    pub name: MachineName,
    /// The names it had before it was renamed, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub was: Vec<MachineName>,
    #[serde(deserialize_with = "crate::tolerant::read")]
    pub kind: MemberKind,
    pub logins: Vec<AccountName>,
    /// The account it is registered under, when it is a machine.
    pub home: Option<AccountName>,
    /// Where other machines connect to it directly, when the network's
    /// administrator allowed that.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<SocketAddr>,
    /// Where it relays its owner's other machines to the hub, when its owner
    /// let it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay: Vec<SocketAddr>,
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
    /// A managed machine: registered under `home`, logged in to nothing
    /// (`world-network.md`, 受管机器).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    /// The token it came in by, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The group its owner put it in (`world-network.md`, 机器的分组).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A group's name as given, made plain: spaces at its ends dropped and
/// those within it made one; none when nothing is left. A group only shows
/// machines together, so any letters do, up to 32 of them.
pub fn group_name(text: &str) -> Result<Option<String>, String> {
    let name = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > 32 {
        return Err("a group's name is at most 32 characters".to_string());
    }
    if name.chars().any(char::is_control) {
        return Err("a group's name has no control characters".to_string());
    }
    Ok(Some(name))
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
    /// What the account may spend of the login in a week; none is no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly: Option<crate::usage::Usd>,
    /// What it may spend in any 24 hours; none is no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<crate::usage::Usd>,
    /// The share of the login's own five-hour window, in percent, past
    /// which the login assists no one; none is no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<u8>,
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
