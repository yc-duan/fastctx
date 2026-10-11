//! Changes an account makes on its hub (`fastctx_net::wire::Change`): which
//! of its machines it shares and which agents' logins it lends, which
//! machines of others still act for it, its devices, tokens and password,
//! its projects; and, for the super administrator, the accounts and the
//! hub's settings (`world-network.md`, 账号只有两级).
//!
//! A change is made as an account, and the member asking must be logged in
//! to it: what an account allows is decided by someone who typed its
//! password on that device, never by a machine that merely acts for it.
//! Opening a project is the one change a machine acting for an account may
//! make for it, as a resident working there opens one.

use fastctx_net::directory::{Account, Lend, Level, Project, Share};
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
            Change::OpenProject { .. } => directory.may_act_for(account, asker),
            _ => directory.logged_in(asker, account),
        };
        if !allowed {
            return Err(format!("{asker} is not logged in to the account {account}"));
        }
        let super_ = found.level == Level::Super;
        let before = matches!(&change, Change::Collab { .. }).then(|| store.book.clone());
        let told = apply(hub, &mut store, asker, account, super_, change, &mut ended)?;
        store.book.directory.settle_acting();
        if let Err(error) = store.save_book() {
            if let Some(before) = before {
                store.book = before;
            }
            return Err(format!("the hub could not keep the change: {error}"));
        }
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
        Change::Collab { collab } => format!("changed model collaboration from version {}", collab.version),
        Change::Share { machine, to, shared } => format!("shares {machine} with {to}: {shared}"),
        Change::Lend { machine, to, agent, home, lent } => format!(
            "lends {agent}'s login {} on {machine} to {to}: {lent}",
            home.as_ref().map(|home| home.display().to_string()).unwrap_or_else(|| "(default)".to_string())
        ),
        Change::StopActing { machine } => format!("{machine} no longer acts for it"),
        Change::Relays { machine, relays } => format!("{machine} connects through {} relays", relays.len()),
        Change::RelayAt { machine, addresses } if addresses.is_empty() => format!("{machine} relays no longer"),
        Change::RelayAt { machine, addresses } => format!(
            "{machine} relays at {}",
            addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        ),
        Change::Remove { name, .. } => format!("removed {name}"),
        Change::Logout => "logged out".to_string(),
        Change::Token { register, expires, .. } => format!("made a token (registers: {register}) until {expires}"),
        Change::PasswordStart { target, .. } => format!("began setting the password of {target}"),
        Change::PasswordFinish { target, .. } => format!("set the password of {target}"),
        Change::OpenAccount { name } => format!("opened the account {name}"),
        Change::SetAccount { name, locked } => format!("set {name}: locked {locked}"),
        Change::Transfer { to } => format!("handed being the super administrator to {to}"),
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
    super_: bool,
    change: Change,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<Told, String> {
    let need_super =
        || if super_ { Ok(()) } else { Err(format!("the account {account} is not the hub's super administrator")) };
    match change {
        Change::Collab { mut collab } => {
            collab.validate()?;
            let found = store.book.directory.accounts.iter_mut().find(|found| found.name == *account).expect("checked");
            let version = found.collab.as_ref().map_or(0, |current| current.version);
            if collab.version != version {
                return Err(format!(
                    "model collaboration changed: version {} was read, current version is {version}",
                    collab.version
                ));
            }
            collab.version = version.checked_add(1).ok_or("model collaboration version cannot be incremented")?;
            let version = collab.version;
            found.collab = Some(collab);
            Ok(Told::CollabWritten { version })
        }
        Change::Share { machine, to, shared } => {
            let directory = &mut store.book.directory;
            let owner = own_machine(directory, account, super_, &machine)?;
            other_account(directory, &owner, &to, &machine)?;
            let share = Share { machine, account: to };
            directory.shares.retain(|existing| *existing != share);
            if shared {
                directory.shares.push(share);
            }
            Ok(Told::Done)
        }
        Change::Lend { machine, to, agent, home, lent } => {
            let directory = &mut store.book.directory;
            let owner = own_machine(directory, account, super_, &machine)?;
            other_account(directory, &owner, &to, &machine)?;
            let lend = Lend { machine, account: to, agent, home };
            directory.lends.retain(|existing| *existing != lend);
            if lent {
                directory.lends.push(lend);
            }
            Ok(Told::Done)
        }
        Change::StopActing { machine } => {
            let directory = &mut store.book.directory;
            directory.acting.retain(|acting| !(acting.account == *account && acting.machine == machine));
            Ok(Told::Done)
        }
        Change::Relays { machine, relays } => {
            let directory = &mut store.book.directory;
            own_machine(directory, account, super_, &machine)?;
            let member = directory.members.iter_mut().find(|member| member.name == machine).expect("checked");
            member.relays = relays;
            Ok(Told::Done)
        }
        Change::RelayAt { machine, addresses } => {
            let directory = &mut store.book.directory;
            own_machine(directory, account, super_, &machine)?;
            let member = directory.members.iter_mut().find(|member| member.name == machine).expect("checked");
            member.relay_at = addresses;
            Ok(Told::Done)
        }
        Change::Remove { name, reason } => {
            let directory = &store.book.directory;
            let member = directory.member(&name).cloned().ok_or_else(|| format!("there is no member {name}"))?;
            if super_ || member.home.as_ref() == Some(account) {
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
                need_super()?;
            }
            if store.book.directory.account(&target).is_none() {
                return Err(format!("there is no account {target}"));
            }
            hub.secrets.register_start(&target, &request).map(|response| Told::RegisterResponse { response })
        }
        Change::PasswordFinish { target, upload } => {
            if target != *account {
                need_super()?;
            }
            if store.book.directory.account(&target).is_none() {
                return Err(format!("there is no account {target}"));
            }
            let record = HubSecrets::register_finish(&upload)?;
            store.book.passwords.insert(target, Bytes(record));
            Ok(Told::Done)
        }
        Change::OpenAccount { name } => {
            need_super()?;
            if store.book.directory.account(&name).is_some() {
                return Err(format!("the account {name} exists already"));
            }
            store.book.directory.accounts.push(Account {
                name,
                level: Level::Member,
                locked: false,
                opened: Timestamp::now(),
                collab: None,
            });
            Ok(Told::Done)
        }
        Change::SetAccount { name, locked } => {
            need_super()?;
            let directory = &mut store.book.directory;
            let found = directory.accounts.iter_mut().find(|found| found.name == name);
            let found = found.ok_or_else(|| format!("there is no account {name}"))?;
            if locked && found.level == Level::Super {
                return Err(format!(
                    "{name} is the hub's super administrator and cannot be locked; hand that to another account first"
                ));
            }
            found.locked = locked;
            Ok(Told::Done)
        }
        Change::Transfer { to } => {
            need_super()?;
            let directory = &mut store.book.directory;
            if to == *account {
                return Err(format!("{to} is the hub's super administrator already"));
            }
            let target = directory.account(&to).ok_or_else(|| format!("there is no account {to}"))?;
            if target.locked {
                return Err(format!("the account {to} is locked"));
            }
            for found in &mut directory.accounts {
                if found.name == to {
                    found.level = Level::Super;
                } else if found.name == *account {
                    found.level = Level::Member;
                }
            }
            Ok(Told::Done)
        }
        Change::DeleteAccount { name } => {
            need_super()?;
            delete_account(store, &name, account, ended)?;
            Ok(Told::Done)
        }
        Change::Settings { self_registration } => {
            need_super()?;
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
            if project.owner != *account && !super_ {
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
            need_super()?;
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

/// Checks `machine` is one of `account`'s machines, or that the super
/// administrator asks; the account it is registered under.
fn own_machine(
    directory: &fastctx_net::directory::Directory,
    account: &AccountName,
    super_: bool,
    machine: &MachineName,
) -> Result<AccountName, String> {
    let member = directory.member(machine).ok_or_else(|| format!("there is no machine {machine}"))?;
    let Some(owner) = member.home.clone() else {
        return Err(format!("{machine} is not registered as a machine"));
    };
    if owner != *account && !super_ {
        return Err(format!("{machine} is not one of the account {account}'s machines"));
    }
    Ok(owner)
}

/// Checks `to` is an account other than `owner`, whose machine `machine` is:
/// within one account nothing is shared or lent.
fn other_account(
    directory: &fastctx_net::directory::Directory,
    owner: &AccountName,
    to: &AccountName,
    machine: &MachineName,
) -> Result<(), String> {
    if directory.account(to).is_none() {
        return Err(format!("there is no account {to}"));
    }
    if to == owner {
        return Err(format!("{machine} is the account {to}'s own"));
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
        forget_machine(directory, name);
    }
    if left {
        remove_member(store, name, by, reason, ended)?;
    }
    Ok(())
}

/// What was shared and lent of `name`, and what it acted for, goes with it
/// as a machine.
fn forget_machine(directory: &mut fastctx_net::directory::Directory, name: &MachineName) {
    directory.shares.retain(|share| share.machine != *name);
    directory.lends.retain(|lend| lend.machine != *name);
    directory.acting.retain(|acting| acting.machine != *name);
}

/// Takes the member `name` off the hub: its logins, what was shared and lent
/// of it and what it acted for go, its key is remembered as removed, and its
/// session ends.
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
    forget_machine(directory, name);
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

/// Deletes `name`: its machines leave the hub, its logins and what it
/// shared, lent and was shared and lent go, its projects are archived whole.
/// The super administrator is not deleted: it hands that on first.
fn delete_account(
    store: &mut Store,
    name: &AccountName,
    by: &AccountName,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<(), String> {
    let directory = &store.book.directory;
    let found = directory.account(name).ok_or_else(|| format!("there is no account {name}"))?;
    if found.level == Level::Super {
        return Err(format!(
            "{name} is the hub's super administrator and cannot be deleted; hand that to another account first"
        ));
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
    book.directory.shares.retain(|share| share.account != *name);
    book.directory.lends.retain(|lend| lend.account != *name);
    book.directory.acting.retain(|acting| acting.account != *name);
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
