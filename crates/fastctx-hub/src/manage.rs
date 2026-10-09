//! Changes an account makes on its hub (`fastctx_net::wire::Change`): who
//! may use its machines, which machines it trusts, its devices, tokens and
//! password, its projects; and, for an administrator, the accounts and the
//! hub's settings.
//!
//! A change is made as an account, and the member asking must be logged in
//! to it: what an account allows is decided by someone who typed its
//! password on that device, never by a machine it merely trusts. Opening a
//! project is the one change a trusted machine may make for an account, as
//! a resident working there opens one.

use fastctx_net::directory::{Account, Grant, Project, Trust};
use fastctx_net::login::HubSecrets;
use fastctx_net::sign::Bytes;
use fastctx_net::token::{Token, TokenSecret};
use fastctx_net::wire::{Change, Refusal, Told};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::store::{Removed, Store, TokenRecord};
use crate::{Hub, Out};

pub(crate) fn change(hub: &Hub, asker: &MachineName, account: &AccountName, change: Change) -> Result<Told, String> {
    let what = describe(&change);
    let mut ended = Vec::new();
    let told = {
        let mut store = hub.store();
        let directory = &store.book.directory;
        let found = directory.account(account).cloned().ok_or_else(|| format!("there is no account {account}"))?;
        if found.locked {
            return Err(format!("the account {account} is locked"));
        }
        let allowed = match &change {
            Change::OpenProject { .. } => directory.trusts(account, asker),
            _ => directory.logged_in(asker, account),
        };
        if !allowed {
            return Err(format!("{asker} is not logged in to the account {account}"));
        }
        let admin = found.admin;
        let told = apply(hub, &mut store, asker, account, admin, change, &mut ended)?;
        store.save_book().map_err(|error| format!("the hub could not keep the change: {error}"))?;
        told
    };
    hub.log.line(format_args!("{asker}, as {account}: {what}"));
    for (name, refusal) in ended {
        if let Some(session) = hub.sessions().get(&name) {
            let _ = session.out.send(Out::End(refusal));
        }
    }
    hub.changed();
    Ok(told)
}

fn describe(change: &Change) -> String {
    match change {
        Change::Grant { machine, to, rights } => {
            format!("{to} may work on {machine}: {}, use its agents: {}", rights.hands, rights.agents)
        }
        Change::Trust { machine, trusted } => format!("trusts {machine}: {trusted}"),
        Change::Relays { machine, relays } => format!("{machine} connects through {} relays", relays.len()),
        Change::Remove { name, .. } => format!("removed {name}"),
        Change::Logout => "logged out".to_string(),
        Change::Token { register, expires, .. } => format!("made a token (registers: {register}) until {expires}"),
        Change::PasswordStart { target, .. } => format!("began setting the password of {target}"),
        Change::PasswordFinish { target, .. } => format!("set the password of {target}"),
        Change::OpenAccount { name, admin, .. } => format!("opened the account {name} (administrator: {admin})"),
        Change::SetAccount { name, admin, may_register, locked } => {
            format!("set {name}: administrator {admin}, may register {may_register}, locked {locked}")
        }
        Change::DeleteAccount { name } => format!("deleted the account {name}"),
        Change::Settings { self_registration } => format!("anyone may open an account: {self_registration}"),
        Change::OpenProject { name, .. } => format!("opened the project {name}"),
        Change::ShareProject { town, with, shared } => format!("shared {town} with {with}: {shared}"),
        Change::MoveProject { town, to } => format!("handed {town} to {to}"),
    }
}

fn apply(
    hub: &Hub,
    store: &mut Store,
    asker: &MachineName,
    account: &AccountName,
    admin: bool,
    change: Change,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<Told, String> {
    let need_admin = || if admin { Ok(()) } else { Err(format!("the account {account} is not an administrator")) };
    match change {
        Change::Grant { machine, to, rights } => {
            let directory = &mut store.book.directory;
            own_machine(directory, account, admin, &machine)?;
            if directory.account(&to).is_none() {
                return Err(format!("there is no account {to}"));
            }
            if directory.member(&machine).and_then(|member| member.home.as_ref()) == Some(&to) {
                return Err(format!("{machine} is the account {to}'s own"));
            }
            directory.grants.retain(|grant| !(grant.machine == machine && grant.account == to));
            if rights.any() {
                directory.grants.push(Grant { machine, account: to, rights });
            }
            Ok(Told::Done)
        }
        Change::Trust { machine, trusted } => {
            let directory = &mut store.book.directory;
            let trust = Trust { account: account.clone(), machine: machine.clone() };
            directory.trust.retain(|existing| *existing != trust);
            if trusted {
                if !directory.rights(account, &machine).any() && !directory.logged_in(&machine, account) {
                    return Err(format!("the account {account} does not use {machine}"));
                }
                directory.trust.push(trust);
            }
            Ok(Told::Done)
        }
        Change::Relays { machine, relays } => {
            let directory = &mut store.book.directory;
            own_machine(directory, account, admin, &machine)?;
            let member = directory.members.iter_mut().find(|member| member.name == machine).expect("checked");
            member.relays = relays;
            Ok(Told::Done)
        }
        Change::Remove { name, reason } => {
            let directory = &store.book.directory;
            let member = directory.member(&name).cloned().ok_or_else(|| format!("there is no member {name}"))?;
            if admin || member.home.as_ref() == Some(account) {
                remove_member(store, &name, Some(account), &reason, ended)?;
            } else if member.logins.contains(account) {
                log_out(store, &name, account, Some(account), &reason, ended)?;
            } else {
                return Err(format!("{name} is neither logged in to nor registered under the account {account}"));
            }
            Ok(Told::Done)
        }
        Change::Logout => {
            log_out(store, asker, account, None, "logged out", ended)?;
            Ok(Told::Done)
        }
        Change::Token { register, name, kind, expires } => {
            if register && !store.book.directory.account(account).is_some_and(|found| found.may_register) {
                return Err(format!("the account {account} may not register machines"));
            }
            if expires <= Timestamp::now() {
                return Err("the token would have expired already".to_string());
            }
            if let Some(name) = &name
                && store.book.directory.member(name).is_some()
            {
                return Err(format!("the name {name} is taken on this hub"));
            }
            let secret = TokenSecret::generate().map_err(|error| format!("making the token: {error}"))?;
            store.book.tokens.push(TokenRecord {
                key: secret.key().public(),
                account: account.clone(),
                register,
                name,
                kind,
                expires,
                made_by: asker.clone(),
                made_at: Timestamp::now(),
                used_by: None,
            });
            let token = Token { addresses: store.hub.public.clone(), cert: hub.pin, hub: store.hub.id.clone(), secret };
            Ok(Told::Token { text: token.to_text() })
        }
        Change::PasswordStart { target, request } => {
            if target != *account {
                need_admin()?;
            }
            if store.book.directory.account(&target).is_none() {
                return Err(format!("there is no account {target}"));
            }
            hub.secrets.register_start(&target, &request).map(|response| Told::RegisterResponse { response })
        }
        Change::PasswordFinish { target, upload } => {
            if target != *account {
                need_admin()?;
            }
            if store.book.directory.account(&target).is_none() {
                return Err(format!("there is no account {target}"));
            }
            let record = HubSecrets::register_finish(&upload)?;
            store.book.passwords.insert(target, Bytes(record));
            Ok(Told::Done)
        }
        Change::OpenAccount { name, admin: as_admin, may_register } => {
            need_admin()?;
            if store.book.directory.account(&name).is_some() {
                return Err(format!("the account {name} exists already"));
            }
            store.book.directory.accounts.push(Account {
                name,
                admin: as_admin,
                may_register,
                locked: false,
                opened: Timestamp::now(),
            });
            Ok(Told::Done)
        }
        Change::SetAccount { name, admin: as_admin, may_register, locked } => {
            need_admin()?;
            let directory = &mut store.book.directory;
            let others =
                directory.accounts.iter().filter(|other| other.name != name && other.admin && !other.locked).count();
            if (!as_admin || locked) && others == 0 {
                return Err("the hub would be left without an administrator".to_string());
            }
            let found = directory.accounts.iter_mut().find(|found| found.name == name);
            let found = found.ok_or_else(|| format!("there is no account {name}"))?;
            found.admin = as_admin;
            found.may_register = may_register;
            found.locked = locked;
            Ok(Told::Done)
        }
        Change::DeleteAccount { name } => {
            need_admin()?;
            delete_account(store, &name, account, ended)?;
            Ok(Told::Done)
        }
        Change::Settings { self_registration } => {
            need_admin()?;
            store.hub.self_registration = self_registration;
            store.save_hub().map_err(|error| format!("the hub could not keep its settings: {error}"))?;
            Ok(Told::Done)
        }
        Change::OpenProject { town, name } => {
            let book = &mut store.book;
            if book.directory.project(&town).is_some() || book.archived.iter().any(|project| project.town == town) {
                return Err(format!("the project {town} exists already"));
            }
            if book.directory.projects.iter().chain(&book.archived).any(|project| project.name == name) {
                return Err(format!("a project named {name} exists on this hub already"));
            }
            book.directory.projects.push(Project { town, name, owner: account.clone(), granted: Default::default() });
            Ok(Told::Done)
        }
        Change::ShareProject { town, with, shared } => {
            let directory = &mut store.book.directory;
            if directory.account(&with).is_none() {
                return Err(format!("there is no account {with}"));
            }
            let project = directory.projects.iter_mut().find(|project| project.town == town);
            let project = project.ok_or_else(|| format!("there is no project {town}"))?;
            if project.owner != *account && !admin {
                return Err(format!("the project {} is the account {}'s", project.name, project.owner));
            }
            if shared {
                project.granted.insert(with);
            } else {
                project.granted.remove(&with);
            }
            Ok(Told::Done)
        }
        Change::MoveProject { town, to } => {
            need_admin()?;
            let book = &mut store.book;
            if book.directory.account(&to).is_none() {
                return Err(format!("there is no account {to}"));
            }
            if let Some(position) = book.archived.iter().position(|project| project.town == town) {
                let mut project = book.archived.remove(position);
                project.owner = to;
                book.directory.projects.push(project);
            } else {
                let project = book.directory.projects.iter_mut().find(|project| project.town == town);
                project.ok_or_else(|| format!("there is no project {town}"))?.owner = to;
            }
            Ok(Told::Done)
        }
    }
}

/// Checks `machine` is one of `account`'s machines, or that an
/// administrator asks.
fn own_machine(
    directory: &fastctx_net::directory::Directory,
    account: &AccountName,
    admin: bool,
    machine: &MachineName,
) -> Result<(), String> {
    let member = directory.member(machine).ok_or_else(|| format!("there is no machine {machine}"))?;
    if !member.is_machine() {
        return Err(format!("{machine} is not registered as a machine"));
    }
    if member.home.as_ref() != Some(account) && !admin {
        return Err(format!("{machine} is not one of the account {account}'s machines"));
    }
    Ok(())
}

/// Takes `account` off the member `name`. Logging out of the account it is
/// registered under unregisters it too; with no login left, it leaves the
/// hub.
fn log_out(
    store: &mut Store,
    name: &MachineName,
    account: &AccountName,
    by: Option<&AccountName>,
    reason: &str,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<(), String> {
    let directory = &mut store.book.directory;
    let member = directory.members.iter_mut().find(|member| member.name == *name);
    let member = member.ok_or_else(|| format!("there is no member {name}"))?;
    member.logins.remove(account);
    let unregistered = member.home.as_ref() == Some(account);
    if unregistered {
        member.home = None;
        member.relays.clear();
    }
    let left = member.logins.is_empty();
    if unregistered {
        directory.grants.retain(|grant| grant.machine != *name);
        directory.trust.retain(|trust| trust.machine != *name);
    }
    if left {
        remove_member(store, name, by, reason, ended)?;
    }
    Ok(())
}

/// Takes the member `name` off the hub: its logins, its grants and the
/// trust in it go, its key is remembered as removed, and its session ends.
fn remove_member(
    store: &mut Store,
    name: &MachineName,
    by: Option<&AccountName>,
    reason: &str,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<(), String> {
    let directory = &mut store.book.directory;
    let position = directory.members.iter().position(|member| member.name == *name);
    let member = directory.members.remove(position.ok_or_else(|| format!("there is no member {name}"))?);
    directory.grants.retain(|grant| grant.machine != *name);
    directory.trust.retain(|trust| trust.machine != *name);
    let at = Timestamp::now();
    store.book.removed.retain(|removed| removed.key != member.key);
    store.book.removed.push(Removed {
        key: member.key,
        name: name.clone(),
        by: by.cloned(),
        at,
        reason: reason.to_string(),
    });
    store.forget_letters(name).map_err(|error| format!("dropping the letters of {name}: {error}"))?;
    ended.push((name.clone(), Refusal::Removed { by: by.cloned(), at, reason: reason.to_string() }));
    Ok(())
}

/// Deletes `name`: its machines leave the hub, its logins and grants go, its
/// projects are archived whole.
fn delete_account(
    store: &mut Store,
    name: &AccountName,
    by: &AccountName,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<(), String> {
    let directory = &store.book.directory;
    let found = directory.account(name).ok_or_else(|| format!("there is no account {name}"))?;
    if found.admin && !directory.accounts.iter().any(|other| other.name != *name && other.admin && !other.locked) {
        return Err("the hub would be left without an administrator".to_string());
    }
    let reason = format!("the account {name} was deleted");
    let machines: Vec<MachineName> = directory
        .members
        .iter()
        .filter(|member| member.home.as_ref() == Some(name))
        .map(|member| member.name.clone())
        .collect();
    for machine in machines {
        remove_member(store, &machine, Some(by), &reason, ended)?;
    }
    let logged: Vec<MachineName> = store
        .book
        .directory
        .members
        .iter()
        .filter(|member| member.logins.contains(name))
        .map(|member| member.name.clone())
        .collect();
    for member in logged {
        log_out(store, &member, name, Some(by), &reason, ended)?;
    }
    let book = &mut store.book;
    book.directory.accounts.retain(|account| account.name != *name);
    book.directory.grants.retain(|grant| grant.account != *name);
    book.directory.trust.retain(|trust| trust.account != *name);
    let (archived, kept): (Vec<Project>, Vec<Project>) =
        std::mem::take(&mut book.directory.projects).into_iter().partition(|project| project.owner == *name);
    book.directory.projects = kept;
    for project in &mut book.directory.projects {
        project.granted.remove(name);
    }
    book.archived.extend(archived);
    book.passwords.remove(name);
    book.tokens.retain(|token| token.account != *name);
    Ok(())
}
