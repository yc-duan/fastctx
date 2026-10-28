//! Changes an account makes on its hub (`fastctx_net::wire::Change`): which
//! of its machines it shares and which agents' logins assist others, which
//! machines of others still act for it, its devices, tokens and password,
//! its projects; and, for the super administrator, the accounts and the
//! hub's settings (`world-network.md`, 账号只有两级).
//!
//! A change is made as an account, and the member asking must be logged in
//! to it: what an account allows is decided by someone who typed its
//! password on that device, never by a machine that merely acts for it.
//! Opening a project is the one change a machine acting for an account may
//! make for it, as a resident working there opens one.

use fastctx_net::directory::{Account, Assist, Level, LoginPlan, Project, Share};
use fastctx_net::login::HubSecrets;
use fastctx_net::sign::{Bytes, PublicKey};
use fastctx_net::token::{Token, TokenSecret};
use fastctx_net::wire::{Change, Refusal, Told};
use fastctx_types::machine_log::{Actor, Outcome};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::store::{Removed, Store, TokenRecord};
use crate::{Hub, Out};

/// Makes `change` as `account`, asked by the member `asker`. Returns the
/// answer, and the sessions the change ends, with why: the caller ends them
/// once the answer is on its way (`end`), so a member that renamed or
/// removed itself hears that it did.
pub(crate) fn change(
    hub: &Hub,
    asker: &MachineName,
    account: &AccountName,
    change: Change,
) -> (Result<Told, String>, Vec<(MachineName, Refusal)>) {
    let mut ended = Vec::new();
    let renaming = matches!(change, Change::Rename { .. });
    let dissolving = matches!(change, Change::Dissolve { .. });
    let deleting = match &change {
        Change::DeleteProject { town } => Some(town.clone()),
        _ => None,
    };
    let told = match made(hub, asker, account, change, &mut ended, renaming) {
        Ok(told) => told,
        Err(reason) => return (Err(reason), Vec::new()),
    };
    if ended.iter().any(|(_, refusal)| matches!(refusal, Refusal::Removed { .. })) {
        let (dir, log) = (hub.store().dir().to_path_buf(), hub.log.clone());
        tokio::task::spawn_blocking(move || {
            if let Err(error) = crate::store::Store::clear_trash(&dir) {
                log.line(format_args!(
                    "deleting the letters of machines taken off: {error}; done when the hub opens next"
                ));
            }
        });
    }
    if let Some(town) = deleting {
        crate::ledger::delete(hub, town);
    }
    hub.changed();
    if dissolving {
        crate::dissolve::changed(hub);
    }
    (Ok(told), ended)
}

/// Ends the sessions a change ended.
pub(crate) fn end(hub: &Hub, ended: Vec<(MachineName, Refusal)>) {
    for (name, refusal) in ended {
        if let Some(session) = hub.sessions().get(&name) {
            let _ = session.out.send(Out::End(refusal));
        }
    }
}

fn made(
    hub: &Hub,
    asker: &MachineName,
    account: &AccountName,
    change: Change,
    ended: &mut Vec<(MachineName, Refusal)>,
    renaming: bool,
) -> Result<Told, String> {
    {
        let mut store = hub.store();
        // An assistance past its deadline is ended first, as the hub's own
        // doing, so no change is made over it: letting that login assist
        // again is a new assistance, which the account uses again before
        // its machine acts for it once more.
        let expired = crate::assists::end_due(&mut store, Timestamp::now())
            .map_err(|error| format!("the hub could not end an assistance past its deadline: {error}"))?;
        let before = store.book.clone();
        let action = crate::machine_log::action(&change, &before, &store.hub.updates);
        let authenticated = may_change(&before.directory, &change, asker, account);
        let actor = Actor { account: Some(account.clone()), authenticated, member: Some(asker.clone()), peer: None };
        let before_hub = store.hub.clone();
        let result = (|| {
            let directory = &before.directory;
            let found = directory.account(account).cloned().ok_or_else(|| format!("there is no account {account}"))?;
            if found.locked {
                return Err(format!("the account {account} is locked"));
            }
            if !may_change(directory, &change, asker, account) {
                return Err(format!("{asker} is not logged in to the account {account}"));
            }
            let super_ = found.level == Level::Super;
            let told = apply(hub, &mut store, asker, account, super_, change, ended)?;
            store.book.directory.settle_acting();
            crate::machine_log::effects(&mut store, &before.directory, &actor, &action)?;
            let committed = match &told {
                Told::CollabWritten { version } => fastctx_types::machine_log::Action::Collab { version: *version },
                Told::ProfileWritten { version } => fastctx_types::machine_log::Action::Profile { version: *version },
                _ => action.clone(),
            };
            crate::machine_log::keep(&mut store, &before.directory, actor.clone(), committed, Outcome::Done)?;
            Ok(told)
        })();
        match result {
            Ok(told) => {
                // What the hub keeps for a renamed machine moves under its
                // new name once the directory that says it is kept; a hub
                // stopped in between moves it when it opens.
                if renaming {
                    match store.settle_names() {
                        Ok(said) => {
                            for line in said {
                                hub.log.line(format_args!("{line}"));
                            }
                        }
                        Err(error) => hub.log.line(format_args!(
                            "moving what is kept for a renamed machine: {error}; it is moved when the hub starts again"
                        )),
                    }
                }
                // Only now that the directory without them is kept do the
                // letters waiting for machines taken off go: a change that
                // was not kept leaves every letter where it was.
                for (name, refusal) in ended.iter() {
                    if matches!(refusal, Refusal::Removed { .. })
                        && let Err(error) = store.forget_letters(name)
                    {
                        hub.log.line(format_args!(
                            "setting aside the letters of {name}, taken off: {error}; done when the hub opens next"
                        ));
                    }
                }
                Ok(told)
            }
            Err(reason) => {
                store.book = before;
                if store.hub != before_hub {
                    store.hub = before_hub;
                    store.save_hub().map_err(|error| format!("restoring the hub's settings: {error}"))?;
                }
                let directory = store.book.directory.clone();
                crate::machine_log::keep(
                    &mut store,
                    &directory,
                    actor,
                    action,
                    Outcome::Refused { reason: reason.clone() },
                )?;
                hub.machine_log_changed.notify_one();
                drop(store);
                if expired {
                    hub.changed();
                }
                Err(reason)
            }
        }
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
        Change::Profile { mut profile } => {
            let directory = &store.book.directory;
            let current = directory.account(account).expect("checked").profile.clone().unwrap_or_default();
            profile.subagents = current.subagents.clone();
            profile.memory = current.memory;
            profile.validate()?;
            // A server a command starts is kept on a machine the account may
            // run commands on; one it was given before stays until removed.
            for (name, entry) in &mut profile.servers {
                let fastctx_types::profile::Entry::Command { machine } = entry else { continue };
                let kept = matches!(
                    current.servers.get(name),
                    Some(fastctx_types::profile::Entry::Command { machine: before }) if before == machine
                );
                if kept {
                    continue;
                }
                if !directory.rights(account, machine).hands {
                    return Err(format!("{account} may not run commands on {machine}"));
                }
                *machine = directory.canonical(machine).clone();
            }
            if profile.version != current.version {
                return Err(format!(
                    "the profile changed: version {} was read, current version is {}",
                    profile.version, current.version
                ));
            }
            profile.version = current.version.checked_add(1).ok_or("the profile version cannot be incremented")?;
            let version = profile.version;
            let found = store.book.directory.accounts.iter_mut().find(|found| found.name == *account).expect("checked");
            found.profile = Some(profile);
            Ok(Told::ProfileWritten { version })
        }
        Change::MemorySettings { version, enabled } => {
            let found = store.book.directory.accounts.iter_mut().find(|found| found.name == *account).expect("checked");
            let mut profile = found.profile.clone().unwrap_or_default();
            if version != profile.version {
                return Err(format!(
                    "the profile changed: version {version} was read, current version is {}",
                    profile.version
                ));
            }
            profile.memory = enabled;
            profile.version = version.checked_add(1).ok_or("the profile version cannot be incremented")?;
            let version = profile.version;
            found.profile = Some(profile);
            Ok(Told::ProfileWritten { version })
        }
        Change::Subagents { version, subagents } => {
            if let Some(config) = &subagents {
                config.validate()?;
            }
            let found = store.book.directory.accounts.iter_mut().find(|found| found.name == *account).expect("checked");
            let mut profile = found.profile.clone().unwrap_or_default();
            if version != profile.version {
                return Err(format!(
                    "the profile changed: version {version} was read, current version is {}",
                    profile.version
                ));
            }
            profile.subagents = subagents;
            profile.version = version.checked_add(1).ok_or("the profile version cannot be incremented")?;
            let version = profile.version;
            found.profile = Some(profile);
            Ok(Told::ProfileWritten { version })
        }
        Change::Share { machine, to, shared, jobs } => {
            let directory = &mut store.book.directory;
            let owner = held_machine(directory, account, &machine)?;
            other_account(directory, &owner, &to, &machine)?;
            if shared && (directory.is_visitor(account) || directory.is_visitor(&owner)) {
                return Err(fastctx_types::sharing::VISITOR_GIVES_NOTHING.into());
            }
            let machine = directory.canonical(&machine).clone();
            if shared && crate::hub_machine::protected(hub, store, &machine) {
                return Err(fastctx_types::sharing::HUB_NOT_SHARED.into());
            }
            let directory = &mut store.book.directory;
            if shared && jobs == Some(0) {
                return Err("the most jobs at once is a number above zero".to_string());
            }
            let share = Share { machine, account: to, jobs: jobs.filter(|_| shared) };
            directory.shares.retain(|existing| existing.machine != share.machine || existing.account != share.account);
            if shared {
                directory.shares.push(share);
            }
            Ok(Told::Done)
        }
        Change::Assist { machine, to, agent, home, assisting, until, weekly, daily, five_hour, turns } => {
            let directory = &mut store.book.directory;
            let owner = held_machine(directory, account, &machine)?;
            other_account(directory, &owner, &to, &machine)?;
            if directory.is_visitor(account) {
                return Err(fastctx_types::sharing::VISITOR_GIVES_NOTHING.into());
            }
            let machine = directory.canonical(&machine).clone();
            if assisting && directory.member(&machine).is_some_and(|member| member.managed) {
                return Err(format!("{machine} is a managed machine: its agents work for no one"));
            }
            if assisting && directory.is_visitor(&owner) {
                return Err(format!("{machine} is a visitor's machine, and a visitor's agents assist no one"));
            }
            if assisting && until.is_some_and(|until| until <= Timestamp::now()) {
                return Err("the assistance's deadline has already passed".to_string());
            }
            if assisting && five_hour.is_some_and(|share| share == 0 || share > 100) {
                return Err("the share of the five-hour window is a percent from 1 to 100".to_string());
            }
            if assisting && daily.is_some_and(|daily| daily.0 == 0) {
                return Err("the daily allowance is an amount above zero".to_string());
            }
            if assisting && turns == Some(0) {
                return Err("the most turns at once is a number above zero".to_string());
            }
            let assist = Assist {
                machine,
                account: to,
                agent,
                home,
                until: until.filter(|_| assisting),
                weekly: weekly.filter(|_| assisting),
                daily: daily.filter(|_| assisting),
                five_hour: five_hour.filter(|_| assisting),
                turns: turns.filter(|_| assisting),
            };
            directory.assists.retain(|existing| !existing.same(&assist));
            if assisting {
                directory.assists.push(assist);
            }
            Ok(Told::Done)
        }
        Change::Plan { machine, agent, home, plan } => {
            need_super()?;
            let directory = &mut store.book.directory;
            if !directory.member(&machine).is_some_and(|member| member.is_machine()) {
                return Err(format!("there is no machine {machine} on this hub"));
            }
            directory.plans.retain(|known| !(known.machine == machine && known.agent == agent && known.home == home));
            if let Some(plan) = plan {
                directory.plans.push(LoginPlan { machine, agent, home, plan });
            }
            Ok(Told::Done)
        }
        Change::StopActing { machine } => {
            let directory = &mut store.book.directory;
            directory.acting.retain(|acting| !(acting.account == *account && acting.machine == machine));
            Ok(Told::Done)
        }
        // The hub checks the machine from outside first, which takes time:
        // the control connection hands it to `crate::direct` instead.
        Change::Direct { .. } => Err("direct connections are allowed through their own check".to_string()),
        Change::Remove { name, reason, then } => {
            let directory = &store.book.directory;
            let member = directory.member(&name).cloned().ok_or_else(|| format!("there is no member {name}"))?;
            if then.is_some() {
                may_uninstall(hub, store, &member, account, super_)?;
            }
            let name = member.name.clone();
            if super_ || directory.may_update(account, &name) {
                remove_member(store, &name, Some(account), &reason, false, then, ended)?;
            } else if then.is_some() {
                return Err(format!("only {name}'s owner takes it off the network to uninstall it"));
            } else if member.logins.contains(account) {
                log_out(store, &name, account, Some(account), &reason, ended)?;
            } else {
                return Err(format!("{name} is neither logged in to nor registered under the account {account}"));
            }
            Ok(Told::Done)
        }
        Change::Rename { machine, to } => {
            rename(store, account, super_, &machine, &to, ended)?;
            Ok(Told::Done)
        }
        Change::Group { machine, group } => {
            let group = group.as_deref().map(fastctx_types::accounts::group_name).transpose()?.flatten();
            own_machine(&store.book.directory, account, super_, &machine)?;
            let key = store.book.directory.member(&machine).expect("own_machine found it").key;
            let member = store.book.directory.members.iter_mut().find(|member| member.key == key).expect("found");
            member.group = group;
            Ok(Told::Done)
        }
        Change::RenameGroup { owner, from, to } => {
            // A group may include machines not shared with the caller:
            // only its owner or administrator renames it (`world-network.md`, 机器的分组).
            let directory = &store.book.directory;
            if owner != *account && (!super_ || directory.is_visitor(&owner)) {
                return Err(format!("the machines of {owner} are not the account {account}'s to group"));
            }
            let to = to.as_deref().map(fastctx_types::accounts::group_name).transpose()?.flatten();
            let mut grouped = 0;
            for member in store.book.directory.members.iter_mut() {
                if member.home.as_ref() == Some(&owner) && member.group.as_deref() == Some(from.as_str()) {
                    member.group = to.clone();
                    grouped += 1;
                }
            }
            if grouped == 0 {
                return Err(format!("{owner} has no group {from}"));
            }
            Ok(Told::Done)
        }
        Change::Logout => {
            log_out(store, asker, account, None, "logged out", ended)?;
            Ok(Told::Done)
        }
        Change::Token { register, name, kind, expires, managed, uses, label, way } => {
            let now = Timestamp::now();
            if expires <= now {
                return Err("the token would have expired already".to_string());
            }
            if managed && !(register && kind.is_computer()) {
                return Err("a token for managed machines registers computers".to_string());
            }
            let uses = uses.unwrap_or(1);
            if uses > crate::store::MOST_USES {
                return Err(format!("a code lets in at most {} devices, or any number", crate::store::MOST_USES));
            }
            if uses != 1 && name.is_some() {
                return Err("a token for several computers names none of them: each takes its own name".to_string());
            }
            let label = label.map(|label| label.trim().to_string()).filter(|label| !label.is_empty());
            if label.as_ref().is_some_and(|label| label.chars().count() > 40) {
                return Err("a code's name is at most 40 characters".to_string());
            }
            if let Some(name) = &name {
                if let Some(taken) = crate::login::name_taken(&store.book.directory, name, None) {
                    return Err(taken);
                }
                if let Some(reason) = crate::login::unread(store, name, None) {
                    return Err(reason);
                }
            }
            let secret = TokenSecret::generate().map_err(|error| format!("making the token: {error}"))?;
            store.book.tokens.retain(|record| !record.spent(now));
            store.book.tokens.push(TokenRecord {
                key: secret.key().public(),
                account: account.clone(),
                register,
                name,
                kind,
                expires,
                managed,
                uses,
                label,
                revoked: false,
                made_by: asker.clone(),
                made_at: now,
                used_by: None,
                used: Vec::new(),
                way,
            });
            // A machine of the account that relays on a network takes one
            // joining there that reaches no further (`world-network.md`,
            // 替局域网里的机器连中枢).
            let relays = store
                .book
                .directory
                .machines()
                .filter(|member| member.home.as_ref() == Some(account))
                .flat_map(|member| member.relay.iter().copied())
                .take(TOKEN_RELAYS)
                .collect();
            let token =
                Token { addresses: store.hub.public.clone(), relays, cert: hub.pin, hub: store.hub.id.clone(), secret };
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
        Change::OpenAccount { name, visitor } => {
            need_super()?;
            if store.book.directory.account(&name).is_some() {
                return Err(format!("the account {name} exists already"));
            }
            store.book.directory.accounts.push(Account {
                name,
                level: if visitor { Level::Visitor } else { Level::Member },
                locked: false,
                opened: Timestamp::now(),
                collab: None,
                profile: None,
                promotion: false,
            });
            Ok(Told::Done)
        }
        Change::SetRole { name, visitor } => {
            need_super()?;
            set_role(store, &name, visitor)?;
            Ok(Told::Done)
        }
        Change::AcceptRole { accept } => {
            let found = store.book.directory.accounts.iter_mut().find(|found| found.name == *account).expect("checked");
            if !found.promotion {
                return Err(format!("the account {account} was not made a member"));
            }
            found.promotion = false;
            if accept {
                found.level = Level::Member;
            }
            Ok(Told::Done)
        }
        Change::RevokeToken { token } => {
            let record = store.book.tokens.iter_mut().find(|record| record.key == token);
            let record = record.ok_or("the hub knows no such token")?;
            if record.account != *account && !super_ {
                return Err(format!("the token was made for the account {}", record.account));
            }
            record.revoked = true;
            let now = Timestamp::now();
            store.book.tokens.retain(|record| !record.spent(now));
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
            if target.level == Level::Visitor {
                return Err(format!("{to} is a visitor; only a member becomes the super administrator"));
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
        Change::Updates { policy } => {
            need_super()?;
            if let Some(minimum) = &policy.minimum
                && fastctx_types::update::release(minimum).is_none()
            {
                return Err(format!("{minimum} is not a version a release can have"));
            }
            // The ceiling on intake is its own change: an interface that
            // sets the rest knows nothing of it.
            let intake = store.hub.updates.intake;
            let channel = store.hub.updates.channel;
            store.hub.updates = fastctx_types::update::Policy { intake, channel, ..policy };
            store.save_hub().map_err(|error| format!("the hub could not keep its settings: {error}"))?;
            Ok(Told::Done)
        }
        Change::Intake { rate } => {
            need_super()?;
            let range = fastctx_types::update::INTAKE_RANGE;
            if rate.is_some_and(|rate| !range.contains(&rate)) {
                return Err(format!(
                    "the hub takes packages in at {} to {} bytes a second",
                    range.start(),
                    range.end()
                ));
            }
            store.hub.updates.intake = rate;
            store.save_hub().map_err(|error| format!("the hub could not keep its settings: {error}"))?;
            Ok(Told::Done)
        }
        // Who may manage a machine may have it update once it is back:
        // holders, and the super administrator but for a visitor's
        // (`world-upgrade.md`, 谁来更新). The update is kept until the
        // machine runs that release or a newer one (`crate::updates`).
        Change::UpdateDue { machine, version, batch } => {
            own_machine(&store.book.directory, account, super_, &machine)?;
            if let Some(version) = &version
                && fastctx_types::update::release(version).is_none()
            {
                return Err(format!("{version} is not a version a release can have"));
            }
            let key = store.book.directory.member(&machine).expect("own_machine found it").key;
            let directory = &store.book.directory;
            if let Some(batch) = &batch
                && (batch.is_empty()
                    || batch.len() > 64
                    || !batch.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)))
            {
                return Err(
                    "an update batch needs an identifier of at most 64 letters, digits, hyphens or underscores".into(),
                );
            }
            store.hub.due.retain(|kept| {
                let replaces = kept.key == key && (version.is_some() || batch.is_none() || kept.batch == batch);
                !replaces && directory.by_key(&kept.key).is_some()
            });
            if let Some(version) = version {
                let manifest = hub.updates.lock().expect("never poisoned").manifest(&version);
                let problem = manifest.is_none().then(|| fastctx_types::update::Problem {
                    reason: fastctx_types::update::FailureReason::Other,
                    why: format!("the manifest of {version} is not available yet"),
                });
                let due = fastctx_types::update::Due {
                    version,
                    by: account.clone(),
                    at: Timestamp::now(),
                    manifest,
                    problem,
                };
                store.hub.due.push(crate::store::DueFor { key, due, batch });
            }
            store.save_hub().map_err(|error| format!("the hub could not keep the update: {error}"))?;
            Ok(Told::Done)
        }
        // The network ends: every machine and device is taken off it, told
        // now or as it next comes that the network is gone; no code lets
        // anything in, and the hub lets nobody in again
        // (`world-network.md`, 解散网络).
        Change::Dissolve { name } => {
            need_super()?;
            if name != store.hub.name {
                return Err(format!("the network is called {}, not {name}", store.hub.name));
            }
            let members: Vec<(MachineName, PublicKey)> =
                store.book.directory.members.iter().map(|member| (member.name.clone(), member.key)).collect();
            for (member, _) in &members {
                remove_member(store, member, Some(account), "the network was dissolved", true, None, ended)?;
            }
            store.book.tokens.clear();
            let untold = members.into_iter().map(|(_, key)| key).collect();
            store.hub.dissolved = Some(crate::store::Dissolved { by: account.clone(), at: Timestamp::now(), untold });
            store
                .save_hub()
                .map_err(|error| format!("the hub could not keep that its network is dissolved: {error}"))?;
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
            // A project is its owner's to share, the super administrator's
            // no more than anyone's (`world-network.md`, 项目也照同一个规矩).
            if project.owner != *account {
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
            let book = &mut store.book;
            if book.directory.account(&to).is_none() {
                return Err(format!("there is no account {to}"));
            }
            if let Some(position) = book.archived.iter().position(|project| project.town == town) {
                // Archived with a deleted account, a project is the super
                // administrator's to hand on.
                need_super()?;
                let mut project = book.archived.remove(position);
                project.owner = to;
                book.directory.projects.push(project);
            } else {
                let project = book.directory.projects.iter_mut().find(|project| project.town == town);
                let project = project.ok_or_else(|| format!("there is no project {town}"))?;
                if project.owner != *account {
                    return Err(format!("the project {} is the account {}'s", project.name, project.owner));
                }
                project.granted.remove(&to);
                project.owner = to;
            }
            Ok(Told::Done)
        }
        Change::Relay { machine, at } => {
            let directory = &mut store.book.directory;
            held_machine(directory, account, &machine)?;
            let machine = directory.canonical(&machine).clone();
            let member = directory.members.iter_mut().find(|member| member.name == machine);
            let member = member.ok_or_else(|| format!("there is no machine named {machine}"))?;
            if !member.is_machine() {
                return Err(format!("{machine} is a device, not a machine; only machines relay"));
            }
            // Holders choose the relay (`world-network.md`, 共享机器与智能体协助).
            // It still carries the owner's machines; the hub checks each one.
            if !at.iter().all(relay_reachable) {
                return Err("a relay is at an address of its network, with a port".to_string());
            }
            let mut addresses = Vec::new();
            for address in at {
                if !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
            if addresses.len() > RELAY_ADDRESSES {
                return Err(format!("a relay is at {RELAY_ADDRESSES} addresses at most"));
            }
            member.relay = addresses;
            Ok(Told::Done)
        }
        Change::DeleteProject { town } => {
            let projects = &mut store.book.directory.projects;
            let position = projects.iter().position(|project| project.town == town);
            let position = position.ok_or_else(|| format!("there is no project {town}"))?;
            if projects[position].owner != *account {
                return Err(format!(
                    "the project {} is the account {}'s",
                    projects[position].name, projects[position].owner
                ));
            }
            // Its ledger goes once the directory without it is kept
            // (`change`).
            let project = projects[position].clone();
            crate::project_deletions::begin(store, project)?;
            store.book.directory.projects.remove(position);
            Ok(Told::Done)
        }
    }
}

/// Whether `asker` may make a change as `account`: a device logged in to
/// it; a machine acting for it opens projects for it. A managed machine,
/// logged in to nothing, makes none, leaving among them: its owner takes it
/// off (`world-network.md`, 受管机器).
/// The addresses one machine relays at, at most.
const RELAY_ADDRESSES: usize = 8;

/// The relays a join code carries, at most: a code names 255 addresses at
/// most, and a machine joining tries each in turn.
const TOKEN_RELAYS: usize = 16;

/// Whether another machine on a network may reach a relay at `address`: an
/// address of one of its cards, with a port, not one only the relay itself
/// reaches or that names no one machine. An IPv6 link-local address is the
/// relay's own card's, which a machine elsewhere names by its own.
fn relay_reachable(address: &std::net::SocketAddr) -> bool {
    let ip = match address.ip() {
        std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(std::net::IpAddr::V6(ip), std::net::IpAddr::V4),
        ip => ip,
    };
    let names_one = match ip {
        std::net::IpAddr::V4(ip) => !ip.is_broadcast() && !ip.is_multicast(),
        std::net::IpAddr::V6(ip) => !ip.is_multicast() && !ip.is_unicast_link_local(),
    };
    address.port() != 0 && !ip.is_unspecified() && !ip.is_loopback() && names_one
}

fn may_change(
    directory: &fastctx_net::directory::Directory,
    change: &Change,
    asker: &MachineName,
    account: &AccountName,
) -> bool {
    match change {
        Change::OpenProject { .. } | Change::Profile { .. } | Change::Subagents { .. } => {
            directory.may_act_for(account, asker)
        }
        _ => directory.logged_in(asker, account),
    }
}

/// Makes `name` a visitor, at once: what it shared and let assist is
/// taken back, as a visitor gives nothing of its machines. Or offers to
/// make it a member, which it becomes once it agrees: a member's machines
/// are the super administrator's to use, and that is not put on anyone
/// unasked (`world-network.md`, 超级管理员管什么).
fn set_role(store: &mut Store, name: &AccountName, visitor: bool) -> Result<(), String> {
    let directory = &mut store.book.directory;
    let found = directory.accounts.iter_mut().find(|found| found.name == *name);
    let found = found.ok_or_else(|| format!("there is no account {name}"))?;
    match (found.level, visitor) {
        (Level::Super, _) => Err(format!("{name} is the hub's super administrator; hand that to a member first")),
        (Level::Visitor, true) => {
            found.promotion = false;
            Ok(())
        }
        (Level::Member, false) => Err(format!("{name} is a member already")),
        (Level::Visitor, false) => {
            found.promotion = true;
            Ok(())
        }
        (Level::Member, true) => {
            found.level = Level::Visitor;
            found.promotion = false;
            let machines: Vec<MachineName> = directory
                .members
                .iter()
                .filter(|member| member.home.as_ref() == Some(name))
                .map(|member| member.name.clone())
                .collect();
            directory.shares.retain(|share| !machines.contains(&share.machine));
            directory.assists.retain(|assist| !machines.contains(&assist.machine));
            Ok(())
        }
    }
}

/// Checks management authority; returns the registered owner.
fn own_machine(
    directory: &fastctx_net::directory::Directory,
    account: &AccountName,
    _super: bool,
    machine: &MachineName,
) -> Result<AccountName, String> {
    let member = directory.member(machine).ok_or_else(|| format!("there is no machine {machine}"))?;
    let Some(owner) = member.home.clone() else {
        return Err(format!("{machine} is not registered as a machine"));
    };
    // The super administrator's reach stops at a visitor's machines.
    if !directory.may_update(account, machine) {
        return Err(format!("{machine} is not one of the account {account}'s machines"));
    }
    Ok(owner)
}

fn held_machine(
    directory: &fastctx_net::directory::Directory,
    account: &AccountName,
    machine: &MachineName,
) -> Result<AccountName, String> {
    directory.authorize(account, fastctx_net::authorization::Action::Hold(machine)).map_err(|denial| denial.message)?;
    Ok(directory.member(machine).and_then(|member| member.home.clone()).expect("holder has a machine"))
}

/// Renames a machine the account manages to `to`
/// (`world-network.md`, 机器的名字). The names it
/// had keep naming it, so nothing written under one is lost: a letter
/// waiting, another machine's records, a script. It takes the new name the
/// next time it connects: a session open now ends, and it connects again at
/// once; one away takes it when it is back. A machine whose release would
/// not take a name it did not ask for is not renamed.
fn rename(
    store: &mut Store,
    account: &AccountName,
    super_: bool,
    machine: &MachineName,
    to: &MachineName,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<(), String> {
    let directory = &store.book.directory;
    own_machine(directory, account, super_, machine)?;
    let member = directory.member(machine).expect("own_machine found it");
    if member.name.as_str() == to.as_str() {
        return Err(format!("{machine} is called {to} already"));
    }
    if let Some(taken) = crate::login::name_taken(directory, to, Some(&member.key)) {
        return Err(taken);
    }
    if let Some(reason) = crate::login::unread(store, to, Some(&member.key)) {
        return Err(reason);
    }
    let old = member.name.clone();
    if !store.letters.get(&old).is_some_and(|letters| letters.renames) {
        return Err(format!(
            "{old} has not connected with a release of fastctx that takes a new name; update fastctx there and let it connect first"
        ));
    }
    store.book.directory.rename(&old, to);
    ended.push((old, Refusal::Renamed { to: to.clone() }));
    Ok(())
}

/// Checks `to` is an account other than `owner`, whose machine `machine` is:
/// within one account nothing is shared and no agent assists.
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
        member.direct.clear();
        member.relay.clear();
    }
    let left = member.logins.is_empty();
    let key = member.key;
    if unregistered {
        forget_machine(directory, name);
        store.hub.due.retain(|due| due.key != key);
        store.hub.package_targets.remove(&key);
        store.save_hub().map_err(|e| format!("removing update arrangements: {e}"))?;
    }
    if left {
        remove_member(store, name, by, reason, false, None, ended)?;
    }
    Ok(())
}

/// What was shared of `name`, the assistance of its agents, and what it
/// acted for go with it as a machine.
fn forget_machine(directory: &mut fastctx_net::directory::Directory, name: &MachineName) {
    directory.shares.retain(|share| share.machine != *name);
    directory.assists.retain(|assist| assist.machine != *name);
    directory.plans.retain(|plan| plan.machine != *name);
    directory.acting.retain(|acting| acting.machine != *name);
}

/// Takes the member `name` off the hub: its logins, what was shared of it,
/// the assistance of its agents and what it acted for go, its key is
/// remembered as removed, and its session ends.
#[allow(clippy::too_many_arguments)]
fn remove_member(
    store: &mut Store,
    name: &MachineName,
    by: Option<&AccountName>,
    reason: &str,
    dissolved: bool,
    then: Option<fastctx_types::uninstall::Then>,
    ended: &mut Vec<(MachineName, Refusal)>,
) -> Result<(), String> {
    let directory = &mut store.book.directory;
    let position = directory.members.iter().position(|member| member.name == *name);
    let member = directory.members.remove(position.ok_or_else(|| format!("there is no member {name}"))?);
    forget_machine(directory, name);
    store.hub.due.retain(|due| due.key != member.key);
    store.hub.package_targets.remove(&member.key);
    store.save_hub().map_err(|e| format!("removing update arrangements: {e}"))?;
    let at = Timestamp::now();
    store.book.removed.retain(|removed| removed.key != member.key);
    store.book.removed.push(Removed {
        key: member.key,
        name: name.clone(),
        by: by.cloned(),
        at,
        reason: reason.to_string(),
        dissolved,
        home: member.home.clone(),
        then,
        left: None,
    });
    // Its letters go once the change is kept (`made`).
    let refusal = Refusal::Removed { by: by.cloned(), at, reason: reason.to_string(), dissolved, then };
    ended.push((name.clone(), refusal));
    Ok(())
}

/// Checks `account` may have `member` uninstall Fastctx as it is taken off
/// (`world-network.md`, 移出网络与卸载): a held machine, or a managed
/// machine the super administrator may manage; never one an
/// application carries Fastctx on, managed or not, which whoever sits at it
/// uninstalls once it is off, nor the machine that runs this hub.
fn may_uninstall(
    hub: &Hub,
    store: &Store,
    member: &fastctx_net::directory::Member,
    account: &AccountName,
    super_: bool,
) -> Result<(), String> {
    let name = &member.name;
    let directory = &store.book.directory;
    if !(directory.holds(account, name) || (super_ && member.managed && directory.may_update(account, name))) {
        return Err(fastctx_types::sharing::HOLDER_REQUIRED.into());
    }
    let running = crate::updates::running_of(hub, &member.key);
    if crate::hub_machine::protected(hub, store, name) {
        return Err(format!("{name} runs this network's hub: move the hub elsewhere or dissolve the network first"));
    }
    if running.as_ref().is_some_and(|running| running.app.is_some()) {
        return Err(format!(
            "Fastctx on {name} is an application: whoever sits at it uninstalls it there once it is off the network"
        ));
    }
    Ok(())
}

/// A machine taken off, with its key `key`, says what it could not do of
/// the uninstall it was asked: kept on its removal, and in the machine log
/// once for each thing it says.
pub(crate) fn left(hub: &Hub, key: &PublicKey, left: &str) {
    let mut store = hub.store();
    let Some(removed) = store.book.removed.iter_mut().rev().find(|removed| removed.key == *key) else { return };
    if removed.left.as_deref() == Some(left) {
        return;
    }
    removed.left = Some(left.to_string());
    let action = fastctx_types::machine_log::Action::UninstallLeft {
        name: removed.name.clone(),
        left: left.to_string(),
        by: removed.by.clone(),
        home: removed.home.clone(),
    };
    let actor = Actor { account: None, authenticated: true, member: None, peer: None };
    let directory = store.book.directory.clone();
    match crate::machine_log::keep(&mut store, &directory, actor, action, Outcome::Done) {
        Ok(()) => hub.machine_log_changed.notify_one(),
        Err(error) => hub.log.line(format_args!("{error}")),
    }
}

/// Deletes `name`: its machines leave the hub, its logins and what it
/// shared and was shared, and the assistance it gave and was given go, its
/// projects are archived whole.
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
        remove_member(store, &machine, Some(by), &reason, false, None, ended)?;
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
    book.directory.assists.retain(|assist| assist.account != *name);
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
