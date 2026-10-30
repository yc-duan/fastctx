//! Who is on a hub and what each may do (`world-network.md`).
//!
//! The hub is where this is decided: accounts and their level, the
//! computers and phones logged in to them, the machines registered under
//! them, the machines shared and the agents' logins assisting other accounts,
//! the machines that act for an account because it used an agent there,
//! and whose each project is. Every member is given its own view of it
//! (`View`), cut to what concerns it, and checks what reaches it against
//! that view before acting: a second look that guards against the hub's
//! mistakes, not against the hub.
//!
//! The judgments are here, on `Directory`, so the hub, which holds all of
//! it, and every member, which holds its cut, make them the same way.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_types::access::{Purpose, Rights, TownId, Use};
use fastctx_types::{AccountName, AgentName, HubName, MachineName, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::HubId;
use crate::sign::PublicKey;

pub use fastctx_types::accounts::{Level, MemberKind as Kind, Paths, Reach};

/// A computer, phone or web AI on the hub, under a name no other member
/// on the hub has, nor had.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: MachineName,
    /// The names it had before it was renamed, oldest first. Each still
    /// names it: what was written under one, a letter waiting, another
    /// machine's records, a script, reaches it (`world-network.md`, 机器的名字).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub was: Vec<MachineName>,
    pub key: PublicKey,
    /// A kind a later hub adds is read as the most careful this release
    /// knows (`fastctx_types::tolerant`).
    #[serde(deserialize_with = "fastctx_types::tolerant::read")]
    pub kind: Kind,
    /// The accounts logged in on it.
    pub logins: BTreeSet<AccountName>,
    /// The account it is registered as a machine under; that account is
    /// among its logins, unless it is a managed machine (`managed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<AccountName>,
    /// A managed machine (`world-network.md`, 受管机器): registered under
    /// `home` by a code, logged in to no account, and never logged in to:
    /// the agents of those who may work on it run commands there; it acts
    /// for no one, no conversation lives on it, its own agents work for no
    /// one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    /// The token it came in by, when one did: those of one code are told to
    /// their owner together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<PublicKey>,
    /// The group its owner put it in: it is shown with the others in that
    /// group and nothing more. A group names no machine to reach, and gives
    /// and takes no one's rights (`world-network.md`, 机器的分组).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Where other machines connect to it directly, when the hub's super
    /// administrator allowed that and the hub reached it there
    /// (`world-network.md`, 直连). Its node listens at their ports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub direct: Vec<std::net::SocketAddr>,
    /// Where it carries its owner's other machines' connections to the hub,
    /// joining ones among them, when its owner let it: addresses on its own
    /// network (`world-network.md`, 替局域网里的机器连中枢). Its node listens
    /// at their ports for that alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay: Vec<std::net::SocketAddr>,
    pub added: Timestamp,
    /// Whether it is connected now, as the hub saw when it made the view.
    #[serde(default)]
    pub online: bool,
    /// When it connected, when online; when it was last connected, when not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Timestamp>,
    /// The machine it reaches the hub through, when online through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    /// What its direct address may be chosen from, as the hub saw when it
    /// made the view; only in views of the super administrator and its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    /// How its lines go, as it last told the hub, while online.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Paths>,
    /// When the hub last set aside what waited for it, it having been away
    /// more than 30 days: a letter to it from before then that was not
    /// answered never will be, nor one from it to a machine away as long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<Timestamp>,
}

/// How a member is connected, as the hub sees it now, as one member's view
/// has it (`Member::online` and what goes with it): told as it changes to a
/// member whose session reads it (`crate::wire::Down::Presence`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<Reach>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Paths>,
}

impl Presence {
    /// `member`'s, as its view has it.
    pub fn of(member: &Member) -> Self {
        Self {
            online: member.online,
            since: member.since,
            through: member.through.clone(),
            reach: member.reach.clone(),
            paths: member.paths.clone(),
        }
    }

    /// Puts it in `member`.
    pub fn apply(&self, member: &mut Member) {
        member.online = self.online;
        member.since = self.since;
        member.through = self.through.clone();
        member.reach = self.reach.clone();
        member.paths = self.paths.clone();
    }
}

impl Member {
    pub fn is_machine(&self) -> bool {
        self.home.is_some()
    }

    /// Whether `name` is its name, now or before.
    pub fn named(&self, name: &MachineName) -> bool {
        self.name == *name || self.was.contains(name)
    }
}

/// A machine shared with an account that is not its own: working on it,
/// and every agent logged in on it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Share {
    pub machine: MachineName,
    pub account: AccountName,
    /// The most jobs the account runs on the machine at once, as its owner
    /// set; none is no limit (`world-network.md`, 共享机器与智能体协助).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<u16>,
}

/// One login of an agent on a machine, assisting an account that is not the
/// machine's: that login works for the account, which is not given the
/// machine; until a deadline, when the owner set one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Assist {
    pub machine: MachineName,
    pub account: AccountName,
    pub agent: AgentName,
    /// The login's directory as the machine writes it; none for the
    /// agent's default login there, which follows its user's environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<PathBuf>,
    /// When the assistance ends by itself; none while the owner keeps it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Timestamp>,
    /// What the account may spend of the login in a week of its own
    /// seven-day window (a Claude Code login's); none is no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly: Option<fastctx_types::usage::Usd>,
    /// What it may spend in any 24 hours (a Claude Code login's); none is
    /// no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<fastctx_types::usage::Usd>,
    /// The share of the login's own five-hour window, in percent, past
    /// which the login assists no one; none is no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<u8>,
    /// The most turns the login runs at once, its owner's and those it
    /// assists together; none is no limit of the owner's (`world-usage.md`,
    /// 一个登录同时开几轮).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<u16>,
}

impl Assist {
    /// Whether this is `agent`'s login `home` there.
    pub fn is(&self, agent: &AgentName, home: Option<&Path>) -> bool {
        self.agent == *agent && self.home.as_deref() == home
    }

    /// Whether `other` is the same login assisting the same account,
    /// whatever its deadline and allowance.
    pub fn same(&self, other: &Assist) -> bool {
        self.machine == other.machine && self.account == other.account && other.is(&self.agent, self.home.as_deref())
    }

    /// Whether it still assists at `now`: its deadline, if any, is later.
    pub fn live(&self, now: Timestamp) -> bool {
        self.until.is_none_or(|until| until > now)
    }
}

/// The plan the hub's super administrator set by hand on one login of an
/// agent on a machine, which names none itself; only shown.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LoginPlan {
    pub machine: MachineName,
    pub agent: AgentName,
    /// The login's directory as the machine writes it; none for the
    /// agent's default login there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<PathBuf>,
    pub plan: fastctx_types::accounts::PlanLabel,
}

/// A machine that acts for an account it is not logged in to: the account
/// used an agent there, opening a session on it or asking it to be a
/// colleague, which needs that machine to reach the account's machines
/// (`world-network.md`, 别人机器上的智能体什么时候能访问你的机器).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActsFor {
    pub machine: MachineName,
    pub account: AccountName,
    pub since: Timestamp,
}

/// A project: the account it was opened in, and the accounts it was
/// shared with, which may read and write its records too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub town: TownId,
    pub name: TownName,
    pub owner: AccountName,
    #[serde(default)]
    pub granted: BTreeSet<AccountName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub name: AccountName,
    /// A level a later hub adds is read as the most careful this release
    /// knows (`fastctx_types::tolerant`).
    #[serde(deserialize_with = "fastctx_types::tolerant::read")]
    pub level: Level,
    pub locked: bool,
    pub opened: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collab: Option<fastctx_types::collab::Collab>,
    /// Its own layer of configuration for the sessions that work for it
    /// (`world-user-config.md`). A machine that does not work for it is
    /// given only the servers on that machine (`Profile::located`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<fastctx_types::profile::Profile>,
    /// A visitor the super administrator made a member, until it agrees
    /// (`world-network.md`, 超级管理员管什么).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub promotion: bool,
}

impl Account {
    /// Whether it is the hub's super administrator, and not locked.
    pub fn is_super(&self) -> bool {
        self.level == Level::Super && !self.locked
    }
}

/// Accounts, members and what each may do: all of it on the hub, a cut of
/// it on each member. Each entry is shared by the copies made of it, so a
/// copy costs a pointer an entry, and a change makes a new entry of only
/// what it changes (`Arc::make_mut`): what is the same in two copies is one
/// entry, which the hub's store needs not write again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directory {
    pub accounts: Vec<Arc<Account>>,
    pub members: Vec<Arc<Member>>,
    #[serde(default)]
    pub shares: Vec<Arc<Share>>,
    /// Written `lends`, the name hubs and members of every version read.
    #[serde(default, rename = "lends")]
    pub assists: Vec<Arc<Assist>>,
    /// Plans set by hand on logins that name none.
    #[serde(default)]
    pub plans: Vec<Arc<LoginPlan>>,
    #[serde(default)]
    pub acting: Vec<Arc<ActsFor>>,
    pub projects: Vec<Arc<Project>>,
}

impl Directory {
    /// The member `name` names: its name now, or one it had. No two members
    /// have a name in common, now or before, so at most one does.
    pub fn member(&self, name: &MachineName) -> Option<&Member> {
        self.members.iter().find(|member| member.name == *name).map(|member| &**member).or_else(|| self.formerly(name))
    }

    /// The member that had `name` before it was renamed.
    pub fn formerly(&self, name: &MachineName) -> Option<&Member> {
        self.members.iter().find(|member| member.was.contains(name)).map(|member| &**member)
    }

    /// The name `name` stands for now: the current name of the member it
    /// names, or itself when it names none. Every judgment goes by it, as
    /// what the directory records of a machine is under its current name.
    pub fn canonical<'a>(&'a self, name: &'a MachineName) -> &'a MachineName {
        self.member(name).map_or(name, |member| &member.name)
    }

    /// Whether `one` and `other` name one member, or are one name.
    pub fn same(&self, one: &MachineName, other: &MachineName) -> bool {
        self.canonical(one) == self.canonical(other)
    }

    /// Renames the member `from` names to `to`, with what is recorded of it
    /// as a machine: its former names keep naming it. `to` may be one of its
    /// own former names, which it takes back. The caller checks that no
    /// other member has or had `to`.
    pub fn rename(&mut self, from: &MachineName, to: &MachineName) -> Option<MachineName> {
        let member = Arc::make_mut(self.members.iter_mut().find(|member| member.named(from))?);
        let old = std::mem::replace(&mut member.name, to.clone());
        member.was.retain(|name| name != to);
        if !member.was.contains(&old) && old != *to {
            member.was.push(old.clone());
        }
        for share in &mut self.shares {
            if share.machine == old {
                Arc::make_mut(share).machine = to.clone();
            }
        }
        for assist in &mut self.assists {
            if assist.machine == old {
                Arc::make_mut(assist).machine = to.clone();
            }
        }
        for plan in &mut self.plans {
            if plan.machine == old {
                Arc::make_mut(plan).machine = to.clone();
            }
        }
        for acting in &mut self.acting {
            if acting.machine == old {
                Arc::make_mut(acting).machine = to.clone();
            }
        }
        Some(old)
    }

    pub fn by_key(&self, key: &PublicKey) -> Option<&Member> {
        self.members.iter().find(|member| member.key == *key).map(|member| &**member)
    }

    /// The registered machines.
    pub fn machines(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|member| member.is_machine()).map(|member| &**member)
    }

    pub fn account(&self, name: &AccountName) -> Option<&Account> {
        self.accounts.iter().find(|account| account.name == *name).map(|account| &**account)
    }

    /// Whether `account` is the hub's super administrator, and not locked.
    pub fn is_super(&self, account: &AccountName) -> bool {
        self.account(account).is_some_and(Account::is_super)
    }

    /// Managing a machine has the same authority as working on it
    /// (`world-network.md`, 共享机器与智能体协助).
    pub fn may_update(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.authorize(account, crate::authorization::Action::Manage(machine)).is_ok()
    }

    /// Whether this account holds a machine, rather than merely administering it.
    pub fn holds(&self, account: &AccountName, machine: &MachineName) -> bool {
        self.authorize(account, crate::authorization::Action::Hold(machine)).is_ok()
    }

    /// Whether `account` is a visitor: its machines are its alone, and it
    /// gives nothing of them to anyone (`world-network.md`, 一个中枢，几个账号).
    pub fn is_visitor(&self, account: &AccountName) -> bool {
        self.account(account).is_some_and(|found| found.level == Level::Visitor)
    }

    /// The hub's super administrator.
    pub fn super_account(&self) -> Option<&Account> {
        self.accounts.iter().find(|account| account.level == Level::Super).map(|account| &**account)
    }

    /// Whether `account` is logged in on `member`.
    pub fn logged_in(&self, member: &MachineName, account: &AccountName) -> bool {
        self.member(member).is_some_and(|member| member.logins.contains(account))
    }

    /// Whether `machine` is shared with `account`.
    pub fn shared(&self, account: &AccountName, machine: &MachineName) -> bool {
        let machine = self.canonical(machine);
        self.shares.iter().any(|share| share.account == *account && share.machine == *machine)
    }

    /// The agents' logins on `machine` assisting `account` now. An
    /// assistance past its deadline gives nothing from that moment, on the
    /// hub and on every member alike, before the hub has taken it off the
    /// list.
    pub fn assisting<'a>(
        &'a self,
        account: &'a AccountName,
        machine: &'a MachineName,
    ) -> impl Iterator<Item = &'a Assist> {
        let now = Timestamp::now();
        let machine = self.canonical(machine);
        self.assists
            .iter()
            .filter(move |assist| assist.account == *account && assist.machine == *machine && assist.live(now))
            .map(|assist| &**assist)
    }

    /// What `account` may do on `machine`: everything on its own machines,
    /// on those shared with it and, for the super administrator, on every
    /// member's machine, a visitor's being its own alone; the logins
    /// assisting it alone on another's; nothing on what is no machine.
    pub fn rights(&self, account: &AccountName, machine: &MachineName) -> Rights {
        use crate::authorization::Action;
        Rights {
            hands: self.authorize(account, Action::Work(machine)).is_ok(),
            agents: self.authorize(account, Action::Login { machine, agent: None, home: None }).is_ok(),
        }
    }

    /// Whether `account` may use `agent`'s login `home` on `machine` (none
    /// is that agent's default login there): any login of a machine it may
    /// work on, else one assisting it.
    pub fn may_use_login(
        &self,
        account: &AccountName,
        machine: &MachineName,
        agent: &AgentName,
        home: Option<&Path>,
    ) -> bool {
        self.authorize(account, crate::authorization::Action::Login { machine, agent: Some(agent), home }).is_ok()
    }

    /// Whether `member` may act for `account`: the account is logged in on
    /// it, or used an agent there and may still use the agents there. An
    /// assistance past its deadline ends that from the same moment as the
    /// assistance, before the hub drops the record of the use
    /// (`settle_acting`). A managed machine acts for no one: no agent of
    /// its is used, so `rights` gives none there.
    pub fn may_act_for(&self, account: &AccountName, member: &MachineName) -> bool {
        self.authorize(account, crate::authorization::Action::ActFor(member)).is_ok()
    }

    /// The accounts `member` may act for.
    pub fn acts_for(&self, member: &MachineName) -> BTreeSet<AccountName> {
        let member = self.canonical(member);
        let mut accounts: BTreeSet<AccountName> =
            self.member(member).map(|member| member.logins.clone()).unwrap_or_default();
        accounts
            .extend(self.acting.iter().filter(|acting| acting.machine == *member).map(|acting| acting.account.clone()));
        accounts.retain(|account| self.may_act_for(account, member));
        accounts
    }

    /// Whether a resident of `account` may live on `member`, or be told
    /// things there: the account is logged in on it or may use agents there.
    pub fn hosts(&self, account: &AccountName, member: &MachineName) -> bool {
        (self.logged_in(member, account) && self.may_act_for(account, member)) || self.rights(account, member).agents
    }

    /// Whether `member`, acting for `account`, may `use_` `machine`; when
    /// not, why, in words for whoever is refused.
    pub fn may(
        &self,
        member: &MachineName,
        account: &AccountName,
        machine: &MachineName,
        use_: Use,
    ) -> Result<(), String> {
        let purpose = match use_ {
            Use::Hands => Purpose::Hands,
            Use::Agents => Purpose::Agents,
        };
        self.admits(member, account, machine, purpose)
    }

    /// Whether `from` may say something for `purpose` to `to`, for
    /// `account`; when not, why, in words for whoever is refused.
    pub fn admits(
        &self,
        from: &MachineName,
        account: &AccountName,
        to: &MachineName,
        purpose: Purpose,
    ) -> Result<(), String> {
        self.authorize(account, crate::authorization::Action::Call { from, to, purpose })
            .map_err(|denial| denial.message)
    }

    /// Whether `from` may tell a resident of `account` on `to` what came of
    /// work it was called to do for it: as `admits` says while the account
    /// uses `from`, and after its rights there are taken back too, as long
    /// as the account stands and `to` still hosts it. Jobs already running
    /// there run to their end, and their end still reaches the session that
    /// started them; the hub passes such a letter only for work it called
    /// `from` to do for that resident (`world-network.md`, 收回立刻生效).
    /// `from` need not be in a member's view by then: a machine the account
    /// may no longer use is cut from it, and the hub vouches for the sender.
    pub fn admits_echo(&self, from: &MachineName, account: &AccountName, to: &MachineName) -> Result<(), String> {
        self.admits(from, account, to, Purpose::Echo).or_else(|refused| {
            let stands = self.account(account).is_some_and(|found| !found.locked);
            if stands && self.hosts(account, to) { Ok(()) } else { Err(refused) }
        })
    }

    /// The machines `account` may work on.
    pub fn workable(&self, account: &AccountName) -> impl Iterator<Item = &Member> {
        self.machines().filter(move |machine| self.rights(account, &machine.name).hands)
    }

    /// The first of `accounts` for which `from` may say something for
    /// `purpose` to `to`; `None` when none may.
    pub fn account_for<'a>(
        &self,
        accounts: impl IntoIterator<Item = &'a AccountName>,
        from: &MachineName,
        to: &MachineName,
        purpose: Purpose,
    ) -> Option<AccountName> {
        accounts.into_iter().find(|account| self.admits(from, account, to, purpose).is_ok()).cloned()
    }

    /// Drops what no longer stands of what acts for accounts: a machine acts
    /// for an account that used an agent there only while the account may
    /// still use agents there, so a share or an assistance ended takes it
    /// along, and assisting again needs another use.
    pub fn settle_acting(&mut self) {
        let keep: Vec<bool> =
            self.acting.iter().map(|acting| self.machine_rights(&acting.account, &acting.machine).agents).collect();
        let mut keep = keep.into_iter();
        self.acting.retain(|_| keep.next().expect("one per entry"));
    }

    pub fn project(&self, town: &TownId) -> Option<&Project> {
        self.projects.iter().find(|project| project.town == *town).map(|project| &**project)
    }

    /// The project named `name` that `account` may use.
    pub fn project_named(&self, account: &AccountName, name: &TownName) -> Option<&Project> {
        self.projects
            .iter()
            .find(|project| project.name == *name && self.in_project(account, &project.town))
            .map(|project| &**project)
    }

    /// Whether `account` may read and write the records of `town`.
    pub fn in_project(&self, account: &AccountName, town: &TownId) -> bool {
        self.authorize(account, crate::authorization::Action::Project(town)).is_ok()
    }

    /// Whether `member`, acting for any account it may, may read and write
    /// the records of `town`.
    pub fn member_in_project(&self, member: &MachineName, town: &TownId) -> bool {
        self.acts_for(member).iter().any(|account| self.in_project(account, town))
    }

    /// Full project bodies belong only on devices logged into a project account.
    /// Hosting an assisting agent grants access to the index, not replication.
    pub fn member_keeps_project(&self, member: &MachineName, town: &TownId) -> bool {
        self.member(member).is_some_and(|member| {
            !member.managed
                && member
                    .logins
                    .iter()
                    .any(|account| self.may_act_for(account, &member.name) && self.in_project(account, town))
        })
    }
}

/// One member's view of its hub's directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The hub takes ledger questions in parts and serves large ledger entries.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ledger_parts: bool,
    /// Account-scoped project indices and signed memory reads/writes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_memory: bool,
    /// Relays collaboration-tree calls with their authorization facts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collaboration_tree: bool,
    /// The hub authorizes large calls from control-frame facts and requires a verifying receiver.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub call_details: bool,
    /// Whether this hub supports `Question::MachineLog`. Older hubs omit it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub machine_log: bool,
    /// The hub keeps each account's layer (`Change::Layer`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub layers: bool,
    pub hub: HubId,
    pub hub_name: HubName,
    /// Current hub host, only when visible to this member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_machine: Option<MachineName>,
    /// Committed changes delivered to accounts logged in on this device.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machine_notices: Vec<fastctx_types::sharing::Notice>,
    /// The member this view is for.
    pub me: MachineName,
    /// Whether accounts may be opened by anyone who reaches the hub.
    #[serde(default)]
    pub self_registration: bool,
    /// Whether this hub takes direct connections between machines: their
    /// allowing, the outside check, and what members say of their lines.
    /// Older hubs omit it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub direct_links: bool,
    /// While the hub checks whether it reaches this member at these
    /// addresses, before direct connections to it count: the member
    /// listens at their ports meanwhile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trial: Vec<std::net::SocketAddr>,
    /// Every machine on the hub that allows direct connections, whatever
    /// account it is of: a member whose own line to the hub is down reaches
    /// the hub through one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwarders: Vec<crate::dial::Forwarder>,
    /// Whether this hub can be moved (`crate::moving`): it takes
    /// `Question::HubMove`. Earlier hubs omit it, and would end the session
    /// of a member that asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moving: bool,
    /// Whether this hub renames machines (`crate::wire::Change::Rename`).
    /// Earlier hubs omit it, and would end the session of a member that
    /// asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub renames: bool,
    /// Whether this hub takes `Up::Afoot` and passes on what is afoot on
    /// each machine (`world-network.md`, 近况). Earlier hubs omit it, and
    /// would end the session of a member that sent it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub afoot: bool,
    /// Whether this hub has its members delete what they keep of a
    /// conversation (`crate::wire::Question::Forget`). Earlier hubs omit
    /// it, and would end the session of a member that asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forgets: bool,
    /// Whether this hub registers managed machines (`Member::managed`) by
    /// tokens for them, and takes back a token (`Change::RevokeToken`).
    /// Earlier hubs omit it: they would make a token for a full login, and
    /// end the session of a member that revoked one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub managed: bool,
    /// Whether this hub has visitors (`Level::Visitor`) and takes the
    /// changes of a role (`Change::SetRole`, `Change::AcceptRole`). Earlier
    /// hubs omit it, and would end the session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub roles: bool,
    /// Whether this hub keeps the groups of machines (`Member::group`,
    /// `Change::Group`, `Change::RenameGroup`). Earlier hubs omit it, and
    /// would end the session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub groups: bool,
    /// Whether this hub dissolves its network when its super administrator
    /// asks (`Change::Dissolve`). Earlier hubs omit it, and would end the
    /// session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dissolves: bool,
    /// Whether this hub deletes a project its owner asks it to
    /// (`Change::DeleteProject`). Earlier hubs omit it, and would end the
    /// session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deletes_projects: bool,
    /// Authorizes project entry returned from a resident's running job.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_entry: bool,
    /// Deletes the owner's conversations as part of deleting a project.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deletes_project_conversations: bool,
    /// Projects this member must finish deleting before acknowledging them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_deletions: Vec<Project>,
    /// Whether this hub keeps the machines that relay for their owner's
    /// others on their network (`Member::relay`, `Change::Relay`). Earlier
    /// hubs omit it, and would end the session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lan_relays: bool,
    /// Whether this hub takes packages in one at a time under a ceiling its
    /// super administrator sets (`Change::Intake`). Earlier hubs omit it,
    /// and would end the session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paced_intake: bool,
    /// Whether this hub keeps each account's own configuration
    /// (`Account::profile`, `Change::Profile`). Earlier hubs omit it, and
    /// would end the session of a member that sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub profiles: bool,
    /// Account-scoped records, fetched separately from this view.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub account_records: bool,
    /// Plan operations and their authorized return from remote jobs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conversation_plans: bool,
    /// Versioned plan history and working-copy merges.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plan_copies: bool,
    /// Atomic, versioned multi-file skill folders.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skill_files: bool,
    /// Computers may release individual record subscriptions after withdrawing copies.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub record_keys: bool,
    /// The hub understands the account-scoped goal control call.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_control: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scratch_locations: bool,
    /// The hub relays resident goal commands and requester controls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_commands: bool,
    /// The hub accepts account-level Claude subagent changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subagents: bool,
    /// Whether this hub keeps the network's versions: it takes
    /// `Up::Versions`, answers `Question::Package`, takes `Change::Updates`
    /// and the data connections of packages. Earlier hubs omit it, and would
    /// end the session of a member that sent any of them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub versions: bool,
    /// Supports progress during package requests and cancellable helpers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub package_progress: bool,
    /// Whether this hub keeps updates for machines away until they are back
    /// (`Change::UpdateDue`) and tells its version with the network's.
    /// Earlier hubs omit it, and would end the session of a member that
    /// sent one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub due: bool,
    /// Whether this hub tells a machine it takes off to uninstall Fastctx
    /// (`Change::Remove::then`). Earlier hubs omit it, and would take the
    /// machine off and leave Fastctx on it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uninstalls: bool,
    /// The tokens the accounts logged in on this member made that still
    /// let devices in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<fastctx_types::accounts::TokenSeen>,
    /// Where this member reaches the hub now, given by a hub that was moved
    /// to where it runs (`crate::moving`); a hub never moved gives none, and
    /// the member keeps the addresses it logged in with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_addresses: Vec<std::net::SocketAddr>,
    /// The moves of the hub this member is told of: the super
    /// administrator's devices of every one, the machine a hub moves to of
    /// its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_moves: Vec<fastctx_types::hub_move::HubMove>,
    /// How many times who is online changed on the hub before it made this
    /// view: a change told after it (`crate::wire::Down::Presence`) counts
    /// past this. An older hub gives none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub presence: u64,
    /// The hub's number of this view among those it gave this member's
    /// session, which a change to it starts from (`crate::view_delta`).
    /// A hub that sends no changes gives none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub revision: u64,
    #[serde(flatten)]
    pub directory: Directory,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

impl View {
    /// An empty view, before the hub has given one.
    pub fn empty(hub: HubId, hub_name: HubName, me: MachineName) -> Self {
        Self {
            ledger_parts: false,
            project_memory: false,
            collaboration_tree: false,
            call_details: false,
            machine_log: false,
            layers: false,
            hub,
            hub_name,
            hub_machine: None,
            machine_notices: Vec::new(),
            me,
            self_registration: false,
            direct_links: false,
            trial: Vec::new(),
            forwarders: Vec::new(),
            moving: false,
            renames: false,
            afoot: false,
            forgets: false,
            managed: false,
            roles: false,
            groups: false,
            dissolves: false,
            deletes_projects: false,
            project_entry: false,
            deletes_project_conversations: false,
            project_deletions: Vec::new(),
            lan_relays: false,
            paced_intake: false,
            profiles: false,
            account_records: false,
            conversation_plans: false,
            plan_copies: false,
            skill_files: false,
            record_keys: false,
            goal_control: false,
            scratch_locations: false,
            goal_commands: false,
            subagents: false,
            versions: false,
            package_progress: false,
            due: false,
            uninstalls: false,
            tokens: Vec::new(),
            hub_addresses: Vec::new(),
            hub_moves: Vec::new(),
            presence: 0,
            revision: 0,
            directory: Directory::default(),
        }
    }

    pub fn me(&self) -> Option<&Member> {
        self.directory.member(&self.me)
    }

    /// Whether `name` names this member: its name now, or one it had.
    pub fn is_me(&self, name: &MachineName) -> bool {
        *name == self.me || self.me().is_some_and(|me| me.named(name))
    }

    /// The accounts this member may act for.
    pub fn mine(&self) -> BTreeSet<AccountName> {
        self.directory.acts_for(&self.me)
    }
}

impl std::ops::Deref for View {
    type Target = Directory;

    fn deref(&self) -> &Directory {
        &self.directory
    }
}
