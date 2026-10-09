//! Account changes and the accounts view, as computers and phones alike
//! make and show them (`fastctx_types::accounts`).

use fastctx_types::AccountName;
use fastctx_types::accounts::{
    AccountChange, AccountSeen, GrantSeen, HubAccounts, MemberSeen, ProjectSeen, TrustSeen,
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
            AccountChange::Grant { machine, to, rights } => Change::Grant { machine, to, rights },
            AccountChange::Trust { machine, trusted } => Change::Trust { machine, trusted },
            AccountChange::Relays { machine, relays } => Change::Relays { machine, relays },
            AccountChange::Remove { name, reason } => Change::Remove { name, reason },
            AccountChange::Logout => Change::Logout,
            AccountChange::Token { register, name, kind, expires } => Change::Token { register, name, kind, expires },
            AccountChange::SetPassword { target, password } => {
                self.set_password(account, &target, &password).await?;
                return Ok(None);
            }
            AccountChange::OpenAccount { name, admin, may_register, password } => {
                self.manage(account, Change::OpenAccount { name: name.clone(), admin, may_register }).await?;
                self.set_password(account, &name, &password).await?;
                return Ok(None);
            }
            AccountChange::SetAccount { name, admin, may_register, locked } => {
                Change::SetAccount { name, admin, may_register, locked }
            }
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
            link: self.state(),
            logins: me.map(|me| me.logins.iter().cloned().collect()).unwrap_or_default(),
            home: me.and_then(|me| me.home.clone()),
            self_registration: view.self_registration,
            accounts: view
                .accounts
                .iter()
                .map(|account| AccountSeen {
                    name: account.name.clone(),
                    admin: account.admin,
                    may_register: account.may_register,
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
            grants: view
                .grants
                .iter()
                .map(|grant| GrantSeen { machine: grant.machine.clone(), account: grant.account.clone(), rights: grant.rights })
                .collect(),
            trust: view
                .trust
                .iter()
                .map(|trust| TrustSeen { account: trust.account.clone(), machine: trust.machine.clone() })
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
