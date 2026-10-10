//! Account changes and the accounts view, as computers and phones alike
//! make and show them (`fastctx_types::accounts`).

use fastctx_types::AccountName;
use fastctx_types::accounts::{
    AccountChange, AccountSeen, ActingSeen, HubAccounts, LendSeen, MemberSeen, ProjectSeen, ShareSeen,
};
use fastctx_types::world::LinkState;

use super::{Link, Status};
use crate::wire::{Change, Told};

impl Link {
    /// How the link stands, as the interfaces show it.
    pub fn state(&self) -> LinkState {
        match self.status() {
            Status::Connecting { since, last_error } => LinkState::Connecting { since, last_error },
            Status::Online { since, address, relay, route } => {
                LinkState::Online { since, hub: address.to_string(), relay, route }
            }
            Status::Stopped { reason } => LinkState::Stopped { reason },
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
            AccountChange::Lend { machine, to, agent, home, lent } => Change::Lend { machine, to, agent, home, lent },
            AccountChange::StopActing { machine } => Change::StopActing { machine },
            AccountChange::Relays { machine, relays } => Change::Relays { machine, relays },
            AccountChange::Remove { name, reason } => Change::Remove { name, reason },
            AccountChange::Logout => Change::Logout,
            AccountChange::Token { register, name, kind, expires } => Change::Token { register, name, kind, expires },
            AccountChange::SetPassword { target, password } => {
                self.set_password(account, &target, &password).await?;
                return Ok(None);
            }
            AccountChange::OpenAccount { name, password } => {
                self.manage(account, Change::OpenAccount { name: name.clone() }).await?;
                self.set_password(account, &name, &password).await?;
                return Ok(None);
            }
            AccountChange::SetAccount { name, locked } => Change::SetAccount { name, locked },
            AccountChange::Transfer { to } => Change::Transfer { to },
            AccountChange::DeleteAccount { name } => Change::DeleteAccount { name },
            AccountChange::Settings { self_registration } => Change::Settings { self_registration },
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
            addresses: self.membership().addresses.clone(),
            link: self.state(),
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
                })
                .collect(),
            members: view
                .members
                .iter()
                .map(|member| MemberSeen {
                    name: member.name.clone(),
                    kind: member.kind,
                    logins: member.logins.iter().cloned().collect(),
                    home: member.home.clone(),
                    relays: member.relays.clone(),
                    added: member.added,
                    online: member.online,
                    since: member.since,
                    through: member.through.clone(),
                    key: member.key.to_string().chars().take(8).collect(),
                })
                .collect(),
            shares: view
                .shares
                .iter()
                .map(|share| ShareSeen { machine: share.machine.clone(), account: share.account.clone() })
                .collect(),
            lends: view
                .lends
                .iter()
                .map(|lend| LendSeen {
                    machine: lend.machine.clone(),
                    account: lend.account.clone(),
                    agent: lend.agent.clone(),
                    home: lend.home.clone(),
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
        }
    }
}
