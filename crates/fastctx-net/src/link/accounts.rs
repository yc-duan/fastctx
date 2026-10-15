//! Account changes and the accounts view, as computers and phones alike
//! make and show them (`fastctx_types::accounts`).

use fastctx_types::AccountName;
use fastctx_types::accounts::{
    AccountChange, AccountSeen, ActingSeen, AssistSeen, HubAccounts, MemberSeen, PlanSeen, ProjectSeen, ShareSeen,
};
use fastctx_types::world::LinkState;
use jiff::Timestamp;

use super::{Link, Status};
use crate::directory::Assist;
use crate::wire::{Change, Told};

impl Link {
    /// How the link stands, as the interfaces show it.
    pub fn state(&self) -> LinkState {
        match self.status() {
            Status::Connecting { since, last_error } => LinkState::Connecting { since, last_error },
            Status::Online { since, address, through, route } => {
                LinkState::Online { since, hub: address.to_string(), through, route, ms: self.hub_ms() }
            }
            Status::Stopped { reason } => LinkState::Stopped { reason, dissolved: self.dissolved() },
        }
    }

    /// Makes `change` as `account`. A password is set with both messages
    /// of OPAQUE made here, so it never leaves this device. Returns a token
    /// when the change made one.
    pub async fn account_change(&self, account: &AccountName, change: AccountChange) -> Result<Option<String>, String> {
        let town_id = |name: &fastctx_types::TownName| {
            self.view()
                .projects
                .iter()
                .find(|project| project.name == *name)
                .map(|project| project.town.clone())
                .ok_or_else(|| format!("there is no project {name} on {}", self.hub_name()))
        };
        let wire = match change {
            AccountChange::Share { machine, to, shared } => Change::Share { machine, to, shared },
            AccountChange::Assist { machine, to, agent, home, assisting, until, weekly } => {
                return self
                    .assist(account, Assist { machine, account: to, agent, home, until, weekly }, assisting)
                    .await
                    .map(|()| None);
            }
            AccountChange::Plan { machine, agent, home, plan } => Change::Plan { machine, agent, home, plan },
            AccountChange::StopActing { machine } => Change::StopActing { machine },
            // An older hub would not read the change, and would end the
            // connection that asked it.
            AccountChange::Direct { .. } if !self.view().direct_links => {
                return Err(format!(
                    "the hub {} runs an older fastctx that does not take direct connections",
                    self.hub_name()
                ));
            }
            AccountChange::Direct { machine, at } => Change::Direct { machine, at },
            AccountChange::Remove { name, reason } => Change::Remove { name, reason },
            AccountChange::Rename { .. } if !self.view().renames => {
                return Err(format!("the hub {} runs an older fastctx that renames no machine", self.hub_name()));
            }
            AccountChange::Rename { machine, to } => Change::Rename { machine, to },
            AccountChange::Group { .. } | AccountChange::RenameGroup { .. } if !self.view().groups => {
                return Err(format!(
                    "the hub {} runs an older fastctx that keeps no groups of machines",
                    self.hub_name()
                ));
            }
            AccountChange::Group { machine, group } => Change::Group { machine, group },
            AccountChange::RenameGroup { owner, from, to } => Change::RenameGroup { owner, from, to },
            AccountChange::Logout => Change::Logout,
            AccountChange::Token { managed, uses, label, .. }
                if (managed || uses.is_some() || label.is_some()) && !self.view().managed =>
            {
                return Err(format!(
                    "the hub {} runs an older fastctx that makes no code for managed machines or several computers",
                    self.hub_name()
                ));
            }
            AccountChange::Token { register, name, kind, expires, managed, uses, label, way } => {
                Change::Token { register, name, kind, expires, managed, uses, label, way }
            }
            AccountChange::RevokeToken { .. } if !self.view().managed => {
                return Err(format!("the hub {} runs an older fastctx that takes no token back", self.hub_name()));
            }
            AccountChange::RevokeToken { token } => Change::RevokeToken { token: token.parse()? },
            AccountChange::SetRole { .. } | AccountChange::AcceptRole { .. } if !self.view().roles => {
                return Err(format!("the hub {} runs an older fastctx that has no visitors", self.hub_name()));
            }
            AccountChange::SetRole { name, visitor } => Change::SetRole { name, visitor },
            AccountChange::AcceptRole { accept } => Change::AcceptRole { accept },
            AccountChange::SetPassword { target, password } => {
                self.set_password(account, &target, &password).await?;
                return Ok(None);
            }
            AccountChange::OpenAccount { visitor: true, .. } if !self.view().roles => {
                return Err(format!("the hub {} runs an older fastctx that has no visitors", self.hub_name()));
            }
            AccountChange::OpenAccount { name, password, visitor } => {
                self.manage(account, Change::OpenAccount { name: name.clone(), visitor }).await?;
                self.set_password(account, &name, &password).await?;
                return Ok(None);
            }
            AccountChange::SetAccount { name, locked } => Change::SetAccount { name, locked },
            AccountChange::Transfer { to } => Change::Transfer { to },
            AccountChange::DeleteAccount { name } => Change::DeleteAccount { name },
            AccountChange::Settings { self_registration } => Change::Settings { self_registration },
            AccountChange::Updates { .. } if !self.view().versions => {
                return Err(format!("the hub {} runs an older fastctx that keeps no versions", self.hub_name()));
            }
            AccountChange::Updates { policy } => Change::Updates { policy },
            AccountChange::Dissolve { .. } if !self.view().dissolves => {
                return Err(format!("the hub {} runs an older fastctx that dissolves no network", self.hub_name()));
            }
            AccountChange::Dissolve { name } => Change::Dissolve { name },
            AccountChange::ShareProject { town, with, shared } => {
                Change::ShareProject { town: town_id(&town)?, with, shared }
            }
            AccountChange::MoveProject { town, to } => Change::MoveProject { town: town_id(&town)?, to },
            AccountChange::Seen { .. } => return Err("what the user saw stays on their computer".to_string()),
        };
        match self.manage(account, wire).await? {
            Told::Token { text } => Ok(Some(text)),
            _ => Ok(None),
        }
    }

    /// Lets `assist`'s login assist as `account`, or no longer. A hub from
    /// before assistance had deadlines or weekly allowances drops what it
    /// does not know and keeps the assistance without it; one meant to end,
    /// or to stop at an allowance, must not outlive that, so it is ended at
    /// once and the owner told why.
    async fn assist(&self, account: &AccountName, assist: Assist, assisting: bool) -> Result<(), String> {
        let change = |assisting: bool, until: Option<Timestamp>, weekly| Change::Assist {
            machine: assist.machine.clone(),
            to: assist.account.clone(),
            agent: assist.agent.clone(),
            home: assist.home.clone(),
            assisting,
            until,
            weekly,
        };
        self.manage(account, change(assisting, assist.until, assist.weekly)).await?;
        if !assisting || (assist.until.is_none() && assist.weekly.is_none()) {
            return Ok(());
        }
        let hub = self.hub_name();
        // The view read again after the change: the assistance as asked,
        // the assistance without what a hub of an older version does not
        // know, or not what was just asked (the view could not be read
        // again).
        let kept =
            self.view().assists.iter().find(|found| found.same(&assist)).map(|found| (found.until, found.weekly));
        let why = match kept {
            Some((until, weekly)) if until == assist.until && weekly == assist.weekly => return Ok(()),
            Some((None, _)) if assist.until.is_some() => {
                format!("the hub {hub} keeps no deadline on assistance; upgrade it to assist until a set time")
            }
            Some(_) => {
                format!("the hub {hub} keeps no weekly allowance on assistance; upgrade it to assist up to an amount")
            }
            None => format!("the hub {hub} could not be asked whether it kept the assistance as given; give it again"),
        };
        match self.manage(account, change(false, None, None)).await {
            Ok(_) => Err(format!("{why}. The assistance was ended")),
            Err(error) => Err(format!("{why}. Ending the assistance failed too: {error}; end it by hand")),
        }
    }

    async fn set_password(&self, account: &AccountName, target: &AccountName, password: &str) -> Result<(), String> {
        fastctx_types::password::check(target.as_str(), password)?;
        let (state, request) = crate::login::password_start(password)?;
        let response = match self.manage(account, Change::PasswordStart { target: target.clone(), request }).await? {
            Told::RegisterResponse { response } => response,
            _ => return Err("the hub answered setting the password with something else".to_string()),
        };
        let upload = crate::login::password_finish(state, password, &response)?;
        self.manage(account, Change::PasswordFinish { target: target.clone(), upload }).await?;
        Ok(())
    }

    /// What this member sees of its hub, as the interfaces show it.
    pub fn accounts(&self) -> HubAccounts {
        let view = self.view();
        let me = view.me();
        HubAccounts {
            hub: self.membership().id.clone(),
            name: view.hub_name.clone(),
            addresses: self.hub_addresses(),
            link: self.state(),
            me: Some(view.me.clone()),
            renames: view.renames,
            managed: view.managed,
            roles: view.roles,
            groups: view.groups,
            dissolves: view.dissolves,
            versions: view.versions,
            logins: me.map(|me| me.logins.iter().cloned().collect()).unwrap_or_default(),
            home: me.and_then(|me| me.home.clone()),
            self_registration: view.self_registration,
            accounts: view
                .accounts
                .iter()
                .map(|account| AccountSeen {
                    name: account.name.clone(),
                    level: account.level,
                    locked: account.locked,
                    opened: account.opened,
                    promotion: account.promotion,
                })
                .collect(),
            members: view
                .members
                .iter()
                .map(|member| MemberSeen {
                    name: member.name.clone(),
                    was: member.was.clone(),
                    kind: member.kind,
                    logins: member.logins.iter().cloned().collect(),
                    home: member.home.clone(),
                    direct: member.direct.clone(),
                    added: member.added,
                    online: member.online,
                    since: member.since,
                    through: member.through.clone(),
                    reach: member.reach.clone(),
                    paths: member.paths.clone(),
                    key: member.key.to_string().chars().take(8).collect(),
                    managed: member.managed,
                    code: member.code.map(|code| code.to_string()),
                    group: member.group.clone(),
                })
                .collect(),
            shares: view
                .shares
                .iter()
                .map(|share| ShareSeen { machine: share.machine.clone(), account: share.account.clone() })
                .collect(),
            assists: view
                .assists
                .iter()
                .map(|assist| AssistSeen {
                    machine: assist.machine.clone(),
                    account: assist.account.clone(),
                    agent: assist.agent.clone(),
                    home: assist.home.clone(),
                    until: assist.until,
                    weekly: assist.weekly,
                })
                .collect(),
            plans: view
                .plans
                .iter()
                .map(|plan| PlanSeen {
                    machine: plan.machine.clone(),
                    agent: plan.agent.clone(),
                    home: plan.home.clone(),
                    plan: plan.plan,
                })
                .collect(),
            acting: view
                .acting
                .iter()
                .map(|acting| ActingSeen {
                    machine: acting.machine.clone(),
                    account: acting.account.clone(),
                    since: acting.since,
                })
                .collect(),
            projects: view
                .projects
                .iter()
                .map(|project| ProjectSeen {
                    name: project.name.clone(),
                    owner: project.owner.clone(),
                    granted: project.granted.iter().cloned().collect(),
                })
                .collect(),
            arrivals: Vec::new(),
            links: self.direct_links(),
            tokens: view.tokens.clone(),
        }
    }
}
