//! Account changes and the accounts view, as computers and phones alike
//! make and show them (`fastctx_types::accounts`).

mod admission;

use fastctx_types::AccountName;
use fastctx_types::accounts::{
    AccountChange, AccountSeen, ActingSeen, AssistSeen, HubAccounts, MemberSeen, PlanSeen, ProjectSeen, ShareSeen,
};
use fastctx_types::world::LinkState;

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
        if matches!(&change, AccountChange::UpdateDue { batch: Some(_), .. }) {
            self.inner.refresh().await?;
        }
        self.check_account_change(&change)?;
        let wire = match change {
            AccountChange::Share { machine, to, shared, jobs } => Change::Share { machine, to, shared, jobs },
            AccountChange::Assist {
                machine,
                to,
                agent,
                home,
                assisting,
                until,
                weekly,
                daily,
                five_hour,
                turns,
                share,
                manages,
                purge,
                shown,
            } => {
                return self
                    .assist(
                        account,
                        Assist {
                            machine,
                            account: to,
                            agent,
                            home,
                            until,
                            weekly,
                            daily,
                            five_hour,
                            turns,
                            share,
                            manages,
                            // The hub gives it.
                            grant: None,
                            also: Vec::new(),
                        },
                        assisting,
                        purge,
                        shown,
                    )
                    .await
                    .map(|()| None);
            }
            AccountChange::Plan { machine, agent, home, plan, ultra } => Change::Plan { machine, agent, home, plan, ultra },
            AccountChange::StopActing { machine } => Change::StopActing { machine },
            AccountChange::Direct { machine, at } => Change::Direct { machine, at },
            AccountChange::Remove { name, reason, then } => Change::Remove { name, reason, then },
            AccountChange::Rename { machine, to } => Change::Rename { machine, to },
            AccountChange::Group { machine, group } => Change::Group { machine, group },
            AccountChange::RenameGroup { owner, from, to } => Change::RenameGroup { owner, from, to },
            AccountChange::Logout => Change::Logout,
            AccountChange::Token { register, name, kind, expires, managed, uses, label, way } => {
                Change::Token { register, name, kind, expires, managed, uses, label, way }
            }
            AccountChange::RevokeToken { token } => Change::RevokeToken { token: token.parse()? },
            AccountChange::SetRole { name, visitor } => Change::SetRole { name, visitor },
            AccountChange::AcceptRole { accept } => Change::AcceptRole { accept },
            AccountChange::SetPassword { target, password } => {
                self.set_password(account, &target, &password).await?;
                return Ok(None);
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
            AccountChange::Updates { policy } => Change::Updates { policy },
            AccountChange::Intake { rate } => Change::Intake { rate },
            AccountChange::UpdateDue { machine, version, batch } => Change::UpdateDue { machine, version, batch },
            AccountChange::Dissolve { name } => Change::Dissolve { name },
            AccountChange::AssetSeal { at_start } => Change::AssetSeal { at_start },
            AccountChange::OpenAssets => Change::OpenAssets,
            AccountChange::ShareProject { town, with, shared } => {
                Change::ShareProject { town: town_id(&town)?, with, shared }
            }
            AccountChange::MoveProject { town, to } => Change::MoveProject { town: town_id(&town)?, to },
            AccountChange::DeleteProject { town } => Change::DeleteProject { town: town_id(&town)? },
            AccountChange::Relay { machine, at } => Change::Relay { machine, at },
            AccountChange::Seen { .. } => return Err("what the user saw stays on their computer".to_string()),
        };
        let preparing = match &wire {
            Change::UpdateDue { machine, version: Some(version), batch: Some(_) } => {
                Some((machine.clone(), version.clone()))
            }
            _ => None,
        };
        let answer = self.manage(account, wire).await?;
        if let Some((machine, version)) = preparing {
            match self.inner.ask(crate::wire::Question::PrepareUpdate { machine, version }).await? {
                Told::Done => {}
                _ => return Err("the hub did not confirm the update's packages are ready".into()),
            }
        }
        match answer {
            Told::Token { text } => Ok(Some(text)),
            _ => Ok(None),
        }
    }

    /// Lets `assist`'s login assist as `account`, or no longer. A hub from
    /// before assistance had deadlines or allowances drops what it does not
    /// know and keeps the assistance without it; one meant to end, or to
    /// stop at an allowance, must not outlive that, so it is ended at once
    /// and the owner told why.
    async fn assist(
        &self,
        account: &AccountName,
        mut assist: Assist,
        assisting: bool,
        purge: bool,
        shown: Vec<fastctx_types::access::GrantId>,
    ) -> Result<(), String> {
        // One who manages the login with its owner names the assistance as
        // the hub keeps it, and the hub alone says what of it they change:
        // its machine's logins are not theirs to ask for, nor is another
        // spelling of the login theirs to end.
        let view = self.view();
        if assisting && assist.manages == Some(true) && !view.managing {
            let hub = self.hub_name();
            return Err(format!("the hub {hub} keeps no one managing a login with its owner; upgrade it for that"));
        }
        // Ending one for good is asked only of a hub that deletes what was
        // born under it: anywhere else it would only end it.
        if purge && assisting {
            return Err("an assistance is ended for good, not given so".into());
        }
        if purge && !view.purges {
            let hub = self.hub_name();
            return Err(format!(
                "the hub {hub} does not yet delete what was born under an assistance, which ending one does now; update the hub first"
            ));
        }
        if !view.holds(account, &assist.machine) {
            if purge {
                return Err("only the login's owner ends its assistance for good".into());
            }
            if !view.managing {
                let hub = self.hub_name();
                return Err(format!("the hub {hub} takes a share from no one but the login's owner; upgrade it for that"));
            }
            let change = Change::Assist {
                machine: assist.machine,
                to: assist.account,
                agent: assist.agent,
                home: assist.home,
                assisting,
                until: assist.until,
                weekly: assist.weekly,
                daily: assist.daily,
                five_hour: assist.five_hour,
                turns: assist.turns,
                share: assist.share,
                manages: assist.manages,
                purge: Vec::new(),
                replaces: Vec::new(),
            };
            return self.manage(account, change).await.map(|_| ());
        }
        let homes = if !assisting && self.view().member(&assist.machine).is_some_and(|member| !member.online) {
            let view = self.view();
            let related: Vec<_> = view
                .assists
                .iter()
                .filter(|entry| {
                    entry.machine == assist.machine && entry.account == assist.account && entry.agent == assist.agent
                })
                .collect();
            if !related.is_empty() && !related.iter().any(|entry| entry.home == assist.home) {
                return Err("the machine is offline; revoke the listed login by its stored path, or wait for it to identify the default login".into());
            }
            Vec::new()
        } else {
            self.login_homes(&assist.machine, account, &assist.agent).await?
        };
        assist.home = super::login_identity::canonical(assist.home.as_deref(), &homes);
        let aliases: Vec<_> = self
            .view()
            .assists
            .iter()
            .filter(|entry| {
                entry.machine == assist.machine
                    && entry.account == assist.account
                    && entry.agent == assist.agent
                    && super::login_identity::canonical(entry.home.as_deref(), &homes) == assist.home
                    && (!assisting || entry.home != assist.home)
            })
            .map(|entry| entry.as_ref().clone())
            .collect();
        // A hub that purges takes the spellings replaced in the very change,
        // their grants kept with it, or ended with it for good: never ended
        // one by one, which would end for good what is only spelled anew
        // (codex-87c8bbd9).
        if view.purges {
            let replaces: Vec<_> =
                aliases.iter().filter(|entry| entry.home != assist.home).map(|entry| entry.home.clone()).collect();
            let mut grants = Vec::new();
            if purge {
                // Every grant of every spelling ended, as shown now.
                for entry in view.assists.iter().filter(|entry| {
                    entry.machine == assist.machine
                        && entry.account == assist.account
                        && entry.agent == assist.agent
                        && super::login_identity::canonical(entry.home.as_deref(), &homes) == assist.home
                }) {
                    for grant in entry.grants() {
                        if !grants.contains(grant) {
                            grants.push(grant.clone());
                        }
                    }
                }
                if grants.is_empty() {
                    return Err("the hub keeps no grant for this assistance; look at it again".into());
                }
                // Bound to what its giver was shown as they asked, every
                // spelling's grant with it: one given again since, or another
                // spelling's added since, is not what they ended
                // (codex-87c8bbd9).
                let now: std::collections::BTreeSet<_> = grants.iter().collect();
                if !shown.is_empty() && now != shown.iter().collect() {
                    return Err("the assistance is not as it was shown; look at it again before ending it".into());
                }
            }
            let change = Change::Assist {
                machine: assist.machine.clone(),
                to: assist.account.clone(),
                agent: assist.agent.clone(),
                home: assist.home.clone(),
                assisting,
                until: assist.until,
                weekly: assist.weekly,
                daily: assist.daily,
                five_hour: assist.five_hour,
                turns: assist.turns,
                share: assist.share,
                manages: assist.manages,
                purge: grants,
                replaces,
            };
            // Such a hub keeps every limit an assistance has: what it was
            // asked stands, and nothing is ended for a view not read again.
            return self.manage(account, change).await.map(|_| ());
        }
        for entry in aliases {
            self.manage(
                account,
                Change::Assist {
                    machine: entry.machine,
                    to: entry.account,
                    agent: entry.agent,
                    home: entry.home,
                    assisting: false,
                    until: None,
                    weekly: None,
                    daily: None,
                    five_hour: None,
                    turns: None,
                    share: None,
                    manages: None,
                    purge: Vec::new(),
                    replaces: Vec::new(),
                },
            )
            .await?;
        }
        let change = |assisting: bool, limited: bool| Change::Assist {
            machine: assist.machine.clone(),
            to: assist.account.clone(),
            agent: assist.agent.clone(),
            home: assist.home.clone(),
            assisting,
            until: assist.until.filter(|_| limited),
            weekly: assist.weekly.filter(|_| limited),
            daily: assist.daily.filter(|_| limited),
            five_hour: assist.five_hour.filter(|_| limited),
            turns: assist.turns.filter(|_| limited),
            share: assist.share.filter(|_| limited),
            manages: assist.manages.filter(|_| limited),
            purge: Vec::new(),
            replaces: Vec::new(),
        };
        self.manage(account, change(assisting, true)).await?;
        let limits = (assist.until, assist.weekly, assist.daily, assist.five_hour, assist.turns);
        if !assisting || limits == (None, None, None, None, None) {
            return Ok(());
        }
        let hub = self.hub_name();
        // The view read again after the change: the assistance as asked,
        // the assistance without what a hub of an older version does not
        // know, or not what was just asked (the view could not be read
        // again).
        let kept = self
            .view()
            .assists
            .iter()
            .find(|found| found.same(&assist))
            .map(|found| (found.until, found.weekly, found.daily, found.five_hour, found.turns));
        let why = match kept {
            Some(found) if found == limits => return Ok(()),
            Some((None, ..)) if assist.until.is_some() => {
                format!("the hub {hub} keeps no deadline on assistance; upgrade it to assist until a set time")
            }
            Some((_, weekly, ..)) if weekly != assist.weekly => {
                format!("the hub {hub} keeps no weekly allowance on assistance; upgrade it to assist up to an amount")
            }
            Some(_) => format!(
                "the hub {hub} keeps no daily allowance, five-hour share or limit of turns at once on assistance; upgrade it to assist within them"
            ),
            None => format!("the hub {hub} could not be asked whether it kept the assistance as given; give it again"),
        };
        match self.manage(account, change(false, false)).await {
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
            hub_machine: view.hub_machine.clone(),
            machine_notices: view.machine_notices.clone(),
            // An earlier hub still tells of rules broken; they are not shown.
            troubles: view
                .troubles
                .iter()
                .filter(|trouble| trouble.kind != fastctx_types::trouble::TroubleKind::Violations)
                .cloned()
                .collect(),
            health: view.health,
            renames: view.renames,
            managed: view.managed,
            roles: view.roles,
            groups: view.groups,
            dissolves: view.dissolves,
            deletes_projects: view.deletes_projects,
            lan_relays: view.lan_relays,
            versions: view.versions,
            package_progress: view.package_progress,
            due: view.due,
            uninstalls: view.uninstalls,
            logins: me.map(|me| me.logins.iter().cloned().collect()).unwrap_or_default(),
            history_switch: view.history_switch,
            history: me
                .into_iter()
                .flat_map(|me| &me.logins)
                .filter(|name| view.account(name).is_some_and(|account| account.history))
                .cloned()
                .collect(),
            home: me.and_then(|me| me.home.clone()),
            self_registration: view.self_registration,
            asset_seals: view.asset_sync > 0,
            asset_seal_at_start: view.asset_seal_at_start,
            assets_sealed: view.assets_sealed.clone(),
            synced: view.synced.clone(),
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
                    relay: member.relay.clone(),
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
                .map(|share| ShareSeen {
                    machine: share.machine.clone(),
                    account: share.account.clone(),
                    jobs: share.jobs,
                    pending: share.pending,
                })
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
                    daily: assist.daily,
                    five_hour: assist.five_hour,
                    turns: assist.turns,
                    share: assist.share,
                    manages: Some(assist.manages()),
                    grants: assist.grants().cloned().collect(),
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
                    ultra: plan.ultra,
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
            // The view holds the projects of every account that may work on
            // this machine, to check calls against; its people are shown
            // only those of the accounts logged in here (`world-network.md`,
            // 项目也照同一个规矩).
            projects: view
                .projects
                .iter()
                .filter(|project| {
                    view.me().is_some_and(|me| {
                        me.logins.contains(&project.owner)
                            || project.granted.iter().any(|account| me.logins.contains(account))
                            // Its machine's co-owners' alike, those frozen in
                            // as it left among them (§6.2).
                            || project.frozen.iter().any(|account| me.logins.contains(account))
                            || project
                                .shared_on
                                .as_ref()
                                .is_some_and(|machine| me.logins.iter().any(|account| view.holds(account, machine)))
                    })
                })
                .map(|project| ProjectSeen {
                    name: project.name.clone(),
                    owner: project.owner.clone(),
                    granted: project.granted.iter().cloned().collect(),
                    shared_on: project.shared_on.clone(),
                    frozen: project.frozen.iter().cloned().collect(),
                })
                .collect(),
            arrivals: Vec::new(),
            links: self.direct_links(),
            tokens: view.tokens.clone(),
        }
    }
}
