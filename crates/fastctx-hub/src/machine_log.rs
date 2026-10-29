//! Machine changes are read by the machines page and account members.
//! They share the directory's atomic disk commit, so a completed change
//! and its record survive together. The hub deletes each record after
//! 90 days, including when nobody reads or changes anything.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use fastctx_net::directory::{Account, Directory};
use fastctx_net::sign::PublicKey;
use fastctx_net::wire::{Change, Down, Told};
use fastctx_types::machine_log::{Action, Actor, Outcome, Page, Record};
use fastctx_types::{AccountName, MachineName};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::store::{Book, Store};

const RETENTION: SignedDuration = SignedDuration::from_hours(90 * 24);

pub(crate) async fn expire(hub: &crate::Hub) -> io::Result<()> {
    loop {
        // Nothing changes while the hub hands its records over.
        hub.moving.unfrozen().await;
        let notified = hub.machine_log_changed.notified();
        let expires = {
            let mut store = hub.store();
            // Nothing changes while the hub hands its records over.
            if !hub.moving.frozen() && store.book.machine_log.expire(Timestamp::now()) {
                store.save_book()?;
            }
            store.book.machine_log.expires()
        };
        match expires {
            Some(expires) => {
                let wait = expires.duration_since(Timestamp::now()).as_secs_f64().max(0.0);
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_secs_f64(wait)) => {},
                    () = notified => {},
                }
            }
            None => notified.await,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct History {
    #[serde(default)]
    next: u64,
    #[serde(default)]
    records: Vec<Kept>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Kept {
    record: Record,
    /// An account name reused after deletion is a different reader.
    accounts: BTreeMap<AccountName, Timestamp>,
    /// Device keys preserve visibility when a name is removed and reused.
    members: BTreeSet<PublicKey>,
    /// Notification recipients fixed at commit time, including holders later removed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    notified: BTreeMap<AccountName, Timestamp>,
}

impl History {
    /// Whether a record says the machine with `key` was updated from `from`
    /// to `to`.
    pub(crate) fn updated(&self, key: &PublicKey, from: &str, to: &str) -> bool {
        self.records.iter().any(|kept| {
            kept.members.contains(key)
                && matches!(&kept.record.action, Action::Updated { from: was, to: now, .. } if was == from && now == to)
        })
    }

    pub(crate) fn expire(&mut self, now: Timestamp) -> bool {
        let before = self.records.len();
        self.records.retain(|kept| kept.record.at.checked_add(RETENTION).expect("timestamp in range") > now);
        before != self.records.len()
    }

    pub(crate) fn expires(&self) -> Option<Timestamp> {
        self.records.iter().map(|kept| kept.record.at.checked_add(RETENTION).expect("timestamp in range")).min()
    }

    pub(crate) fn push(
        &mut self,
        directory: &Directory,
        current_accounts: &[std::sync::Arc<Account>],
        actor: Actor,
        action: Action,
        outcome: Outcome,
    ) -> io::Result<()> {
        let notified = if outcome == Outcome::Done {
            notification(&actor, &action)
                .map(|(machine, by, _)| {
                    let machine = directory.canonical(machine);
                    let owner = directory.member(machine).and_then(|member| member.home.as_ref());
                    let mut recipients: BTreeSet<_> = owner.into_iter().cloned().collect();
                    recipients.extend(
                        directory
                            .shares
                            .iter()
                            .filter(|share| share.machine == *machine)
                            .map(|share| share.account.clone()),
                    );
                    if let Action::Share { to, .. } = &action {
                        recipients.insert(to.clone());
                    }
                    recipients
                        .into_iter()
                        .filter(|account| Some(account) != by)
                        .filter_map(|name| directory.account(&name).map(|account| (name, account.opened)))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        let mut accounts: BTreeSet<AccountName> = actor.account.iter().cloned().collect();
        accounts.extend(notified.keys().cloned());
        let mut names: BTreeSet<MachineName> = BTreeSet::new();
        match &action {
            Action::Share { machine, to, .. }
            | Action::Assist { machine, to, .. }
            | Action::AssistEnded { machine, to, .. } => {
                names.insert(machine.clone());
                accounts.insert(to.clone());
            }
            Action::StopActing { machine }
            | Action::Plan { machine, .. }
            | Action::Relays { machine, .. }
            | Action::RelayAt { machine, .. }
            | Action::Direct { machine, .. }
            | Action::AllowActing { machine, .. }
            | Action::AllowSentinelHome { machine } => {
                names.insert(machine.clone());
            }
            Action::Remove { name, .. } => {
                names.insert(name.clone());
            }
            // The machine is off the hub by then: who took it off and whose
            // it was read it.
            Action::UninstallLeft { by, home, .. } => {
                accounts.extend(by.iter().cloned());
                accounts.extend(home.iter().cloned());
            }
            Action::Rename { machine, .. } | Action::Group { machine, .. } | Action::UpdateDue { machine, .. } => {
                names.insert(machine.clone());
            }
            Action::Updated { machine, by, .. } => {
                names.insert(machine.clone());
                accounts.insert(by.clone());
            }
            Action::RenameGroup { owner, .. } => {
                accounts.insert(owner.clone());
            }
            Action::Joined { .. } => {
                names.extend(actor.member.iter().cloned());
            }
            Action::Left { name, account, .. } => {
                names.insert(name.clone());
                accounts.insert(account.clone());
            }
            Action::ActingStopped { machine, account } => {
                names.insert(machine.clone());
                accounts.insert(account.clone());
            }
            Action::Token { name, .. } => {
                names.extend(name.iter().cloned());
            }
            Action::PasswordStart { target } | Action::PasswordFinish { target } => {
                accounts.insert(target.clone());
            }
            Action::OpenAccount { name, .. } | Action::SetAccount { name, .. } | Action::SetRole { name, .. } => {
                accounts.insert(name.clone());
            }
            Action::AcceptRole { .. } | Action::RevokeToken { .. } => {}
            Action::DeleteAccount { name } => {
                accounts.insert(name.clone());
                names.extend(
                    directory
                        .members
                        .iter()
                        .filter(|member| member.home.as_ref() == Some(name) || member.logins.contains(name))
                        .map(|member| member.name.clone()),
                );
                names.extend(
                    directory.shares.iter().filter(|share| share.account == *name).map(|share| share.machine.clone()),
                );
                names.extend(
                    directory
                        .assists
                        .iter()
                        .filter(|assist| assist.account == *name)
                        .map(|assist| assist.machine.clone()),
                );
                for project in &directory.projects {
                    if project.owner == *name || project.granted.contains(name) {
                        accounts.insert(project.owner.clone());
                        accounts.extend(project.granted.iter().cloned());
                    }
                }
            }
            Action::Transfer { to } => {
                accounts.insert(to.clone());
                accounts.extend(directory.accounts.iter().map(|account| account.name.clone()));
            }
            Action::OpenProject { town, .. }
            | Action::ShareProject { town, .. }
            | Action::MoveProject { town, .. }
            | Action::DeleteProject { town, .. } => {
                if let Some(project) = directory.project(town) {
                    accounts.insert(project.owner.clone());
                    accounts.extend(project.granted.iter().cloned());
                }
                match &action {
                    Action::ShareProject { with, .. } => {
                        accounts.insert(with.clone());
                    }
                    Action::MoveProject { to, .. } => {
                        accounts.insert(to.clone());
                    }
                    _ => {}
                }
            }
            Action::Logout => {
                names.extend(actor.member.iter().cloned());
            }
            Action::Settings { .. } | Action::Updates { .. } | Action::Dissolve { .. } => {
                accounts.extend(directory.accounts.iter().map(|account| account.name.clone()));
            }
            Action::Collab { .. } | Action::Profile { .. } | Action::Forget { .. } | Action::Forgotten { .. } => {}
        }
        let mut members = BTreeSet::new();
        for name in names {
            if let Some(member) = directory.member(&name) {
                members.insert(member.key);
                accounts.extend(member.home.iter().cloned());
                if matches!(action, Action::Remove { .. }) {
                    accounts.extend(member.logins.iter().cloned());
                }
            }
            if matches!(
                action,
                Action::Remove { .. } | Action::Left { .. } | Action::DeleteAccount { .. } | Action::Rename { .. }
            ) {
                let machine = directory.canonical(&name);
                accounts.extend(
                    directory
                        .shares
                        .iter()
                        .filter(|share| share.machine == *machine)
                        .map(|share| share.account.clone()),
                );
                accounts.extend(
                    directory
                        .assists
                        .iter()
                        .filter(|assist| assist.machine == *machine)
                        .map(|assist| assist.account.clone()),
                );
            }
        }
        // A conversation's record is its account's alone: the member that
        // asked or did it reads it by that account. A project's is its
        // accounts' alone, whoever else the machine that did it acts for.
        if let Some(member) = actor.member.as_ref().and_then(|name| directory.member(name))
            && !action.of_conversation()
            && !action.of_project()
        {
            members.insert(member.key);
            accounts.extend(member.home.iter().cloned());
        }
        self.next = self.next.checked_add(1).ok_or_else(|| io::Error::other("machine log numbering exhausted"))?;
        let at = Timestamp::now();
        self.expire(at);
        let accounts = accounts
            .into_iter()
            .filter_map(|name| {
                directory
                    .account(&name)
                    .or_else(|| current_accounts.iter().find(|account| account.name == name).map(|account| &**account))
                    .map(|account| (name, account.opened))
            })
            .collect();
        self.records.push(Kept {
            record: Record { id: self.next, at, actor, action, outcome },
            accounts,
            members,
            notified,
        });
        Ok(())
    }

    /// Delivered on the ordinary directory push to all devices logged into each
    /// recipient account, including after reconnect or the machine's removal.
    pub(crate) fn notices(&self, directory: &Directory, me: &MachineName) -> Vec<fastctx_types::sharing::Notice> {
        let Some(member) = directory.member(me) else { return Vec::new() };
        let now = Timestamp::now();
        let mut notices = Vec::new();
        for kept in &self.records {
            if kept.record.at.checked_add(RETENTION).expect("timestamp in range") <= now {
                continue;
            }
            let Some((machine, by, action)) = notification(&kept.record.actor, &kept.record.action) else { continue };
            for account in &member.logins {
                if !directory
                    .account(account)
                    .is_some_and(|found| !found.locked && kept.notified.get(account) == Some(&found.opened))
                {
                    continue;
                }
                notices.push(fastctx_types::sharing::Notice {
                    id: kept.record.id,
                    at: kept.record.at,
                    account: account.clone(),
                    machine: machine.clone(),
                    by: by.cloned(),
                    action: action.clone(),
                });
            }
        }
        notices
    }

    /// The records `me` reads for `account`, newest first. A kind of record
    /// its release does not read is left out (`Action::needs`): it could
    /// read nothing of a page with one. The super administrator reads every
    /// record but those of conversations and projects, which only their
    /// accounts read.
    pub(crate) fn read(
        &self,
        directory: &Directory,
        me: &MachineName,
        account: &AccountName,
        before: Option<u64>,
        reads: fastctx_types::machine_log::Reads,
    ) -> Result<Page, String> {
        if !directory.may_act_for(account, me) {
            return Err(format!("{me} may not read the machine log of the account {account}"));
        }
        let member = directory.member(me).ok_or_else(|| format!("there is no member {me}"))?;
        let whole = directory.logged_in(me, account) && directory.is_super(account);
        let opened = directory.account(account).map(|account| account.opened);
        let now = Timestamp::now();
        let mut page = Page { records: Vec::new(), next_before: None };
        let envelope = Down::Told {
            id: u64::MAX,
            answer: Ok(Told::MachineLog { page: Page { records: Vec::new(), next_before: Some(u64::MAX) } }),
        };
        let mut bytes = fastctx_net::wire::encode(&envelope).len();
        let mut visible = self
            .records
            .iter()
            .rev()
            .filter(|kept| {
                let private = kept.record.action.of_conversation() || kept.record.action.of_project();
                before.is_none_or(|before| kept.record.id < before)
                    && reads.covers(kept.record.action.needs())
                    && kept.record.at.checked_add(RETENTION).expect("timestamp in range") > now
                    && ((whole && !private)
                        || kept.accounts.get(account).copied().is_some_and(|kept| Some(kept) == opened)
                        || kept.members.contains(&member.key))
            })
            .peekable();
        while let Some(kept) = visible.next() {
            // The envelope reserves the largest request id and cursor; each
            // record is encoded once, with its separator counted exactly.
            let added = fastctx_net::wire::encode(&kept.record).len() + usize::from(!page.records.is_empty());
            if bytes + added > fastctx_net::wire::MAX_CONTROL_FRAME {
                let last = page.records.last().ok_or("one machine log record exceeds a control frame")?;
                page.next_before = Some(last.id);
                break;
            }
            bytes += added;
            page.records.push(kept.record.clone());
            page.next_before = visible.peek().map(|_| kept.record.id);
        }
        Ok(page)
    }
}

fn notification<'a>(
    actor: &'a Actor,
    action: &'a Action,
) -> Option<(&'a MachineName, Option<&'a AccountName>, fastctx_types::sharing::Change)> {
    use fastctx_types::sharing::Change as Notice;
    let by = actor.account.as_ref();
    Some(match action {
        Action::Rename { machine, to } => (machine, by, Notice::Rename { to: to.clone() }),
        Action::Updated { machine, from, to, by } => {
            (machine, Some(by), Notice::Updated { from: from.clone(), to: to.clone() })
        }
        Action::Remove { name, then, .. } => (name, by, Notice::Remove { then: *then }),
        Action::Share { machine, to, shared, .. } => (machine, by, Notice::Share { to: to.clone(), shared: *shared }),
        Action::Assist { machine, to, agent, home, assisting, until, .. } => (
            machine,
            by,
            Notice::Assist {
                to: to.clone(),
                agent: agent.clone(),
                home: home.clone(),
                assisting: *assisting,
                until: *until,
            },
        ),
        _ => return None,
    })
}

pub(crate) fn action(change: &Change, book: &Book, policy: &fastctx_types::update::Policy) -> Action {
    let project_name = |town| {
        book.directory
            .projects
            .iter()
            .chain(&book.archived)
            .find(|project| &project.town == town)
            .map(|project| project.name.clone())
    };
    match change {
        Change::Collab { collab } => Action::Collab { version: collab.version },
        Change::Profile { profile } => Action::Profile { version: profile.version },
        Change::Subagents { version, .. } | Change::MemorySettings { version, .. } | Change::Layer { version, .. } => {
            Action::Profile { version: *version }
        }
        Change::Share { machine, to, shared, jobs } => {
            Action::Share { machine: machine.clone(), to: to.clone(), shared: *shared, jobs: jobs.filter(|_| *shared) }
        }
        Change::Assist { machine, to, agent, home, assisting, until, weekly, daily, five_hour, turns } => {
            Action::Assist {
                machine: machine.clone(),
                to: to.clone(),
                agent: agent.clone(),
                home: home.clone(),
                assisting: *assisting,
                until: until.filter(|_| *assisting),
                weekly: weekly.filter(|_| *assisting),
                daily: daily.filter(|_| *assisting),
                five_hour: five_hour.filter(|_| *assisting),
                turns: turns.filter(|_| *assisting),
            }
        }
        Change::Plan { machine, agent, home, plan } => {
            Action::Plan { machine: machine.clone(), agent: agent.clone(), home: home.clone(), plan: *plan }
        }
        Change::StopActing { machine } => Action::StopActing { machine: machine.clone() },
        Change::Direct { machine, at } => Action::Direct { machine: machine.clone(), at: at.clone() },
        Change::Remove { name, reason, then } => {
            Action::Remove { name: name.clone(), reason: reason.clone(), then: *then }
        }
        Change::Rename { machine, to } => Action::Rename { machine: machine.clone(), to: to.clone() },
        Change::Group { machine, group } => Action::Group { machine: machine.clone(), group: group.clone() },
        Change::RenameGroup { owner, from, to } => {
            Action::RenameGroup { owner: owner.clone(), from: from.clone(), to: to.clone() }
        }
        Change::Logout => Action::Logout,
        Change::Token { register, name, kind, expires, managed, label, uses, way } => Action::Token {
            register: *register,
            name: name.clone(),
            kind: *kind,
            expires: *expires,
            managed: *managed,
            label: label.clone(),
            uses: *uses,
            way: *way,
        },
        Change::RevokeToken { token } => {
            let record = book.tokens.iter().find(|record| record.key == *token);
            Action::RevokeToken {
                label: record.and_then(|record| record.label.clone()),
                way: record.and_then(|record| record.way),
                managed: record.is_some_and(|record| record.managed),
            }
        }
        Change::SetRole { name, visitor } => Action::SetRole { name: name.clone(), visitor: *visitor },
        Change::AcceptRole { accept } => Action::AcceptRole { accept: *accept },
        Change::PasswordStart { target, .. } => Action::PasswordStart { target: target.clone() },
        Change::PasswordFinish { target, .. } => Action::PasswordFinish { target: target.clone() },
        Change::OpenAccount { name, visitor } => Action::OpenAccount { name: name.clone(), visitor: *visitor },
        Change::SetAccount { name, locked } => Action::SetAccount { name: name.clone(), locked: *locked },
        Change::Transfer { to } => Action::Transfer { to: to.clone() },
        Change::DeleteAccount { name } => Action::DeleteAccount { name: name.clone() },
        Change::Settings { self_registration } => Action::Settings { self_registration: *self_registration },
        Change::Updates { policy } => Action::Updates {
            channel: policy.channel,
            minimum: policy.minimum.clone(),
            managed_auto: Some(policy.managed_auto),
            intake: None,
        },
        Change::Intake { rate } => Action::Updates {
            channel: policy.channel,
            minimum: policy.minimum.clone(),
            managed_auto: Some(policy.managed_auto),
            intake: Some(rate.unwrap_or(fastctx_types::update::INTAKE)),
        },
        Change::UpdateDue { machine, version, .. } => {
            Action::UpdateDue { machine: machine.clone(), version: version.clone() }
        }
        Change::Dissolve { name } => Action::Dissolve { name: name.clone() },
        Change::OpenProject { town, name } => Action::OpenProject { town: town.clone(), name: name.clone() },
        Change::ShareProject { town, with, shared } => {
            Action::ShareProject { town: town.clone(), name: project_name(town), with: with.clone(), shared: *shared }
        }
        Change::MoveProject { town, to } => {
            Action::MoveProject { town: town.clone(), name: project_name(town), to: to.clone() }
        }
        Change::DeleteProject { town } => Action::DeleteProject { town: town.clone(), name: project_name(town) },
        Change::Relay { machine, at } => Action::RelayAt { machine: machine.clone(), addresses: at.clone() },
    }
}

pub(crate) fn keep(
    store: &mut Store,
    directory: &Directory,
    actor: Actor,
    action: Action,
    outcome: Outcome,
) -> Result<(), String> {
    // An unrelated login can still remove just its login from a device. That
    // does not remove the machine and must not tell its holders that it did.
    let login_only = matches!(&action, Action::Remove { name, .. } if store.book.directory.member(name).is_some());
    store
        .book
        .machine_log
        .push(directory, &store.book.directory.accounts, actor, action, outcome)
        .map_err(|error| format!("the hub could not keep the machine log: {error}"))?;
    if login_only && let Some(kept) = store.book.machine_log.records.last_mut() {
        kept.notified.clear();
    }
    store.save_book().map_err(|error| format!("the hub could not keep the machine log: {error}"))
}

pub(crate) fn effects(store: &mut Store, before: &Directory, actor: &Actor, action: &Action) -> Result<(), String> {
    let after = &store.book.directory;
    let mut effects = Vec::new();
    for member in &before.members {
        let current = after.member(&member.name);
        for account in &member.logins {
            if current.is_none_or(|current| !current.logins.contains(account)) {
                let reason = match action {
                    Action::Remove { reason, .. } => reason.clone(),
                    Action::DeleteAccount { name } => format!("the account {name} was deleted"),
                    _ => "logged out".to_string(),
                };
                effects.push(Action::Left {
                    name: member.name.clone(),
                    account: account.clone(),
                    kind: member.kind,
                    unregistered: member.home.as_ref() == Some(account),
                    reason,
                });
            }
        }
    }
    for acting in &before.acting {
        if !after
            .acting
            .iter()
            .any(|current| current.account == acting.account && after.same(&current.machine, &acting.machine))
            && !matches!(action, Action::StopActing { machine } if machine == &acting.machine && actor.account.as_ref() == Some(&acting.account))
        {
            effects.push(Action::ActingStopped { machine: acting.machine.clone(), account: acting.account.clone() });
        }
    }
    for effect in effects {
        store
            .book
            .machine_log
            .push(before, &store.book.directory.accounts, actor.clone(), effect, Outcome::Done)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
