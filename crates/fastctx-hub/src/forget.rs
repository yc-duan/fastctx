//! Deleting a conversation from every member keeping something of it
//! (`world-colleagues.md`, 用户在任何地方和居民说话): the machine it lives
//! on, the machines it worked on, and the computers and phones that may
//! keep a copy of it from showing it. The hub keeps each deletion until each
//! of them says it is done, telling each now when it is connected and again
//! at every session it opens, as with a letter; the machine log says which
//! did and which are still waited for.
//!
//! The machine the conversation lives on is told first, alone: only it can
//! say the conversation works for the account the deletion is for, so
//! nothing of it goes elsewhere before that machine did its part. A deletion
//! its machine itself asks for starts past that, as it deleted what it kept
//! before asking. While a deletion is kept, no member is told of the
//! conversation as afoot.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fastctx_net::wire::{Down, Forgetting, Forgot, Told};
use fastctx_types::machine_log::{Action, Actor, Outcome};
use fastctx_types::{AccountName, Address, MachineName, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::store::Store;
use crate::{Hub, Out};

/// The deletions the hub keeps, with the directory (`crate::store`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Forgettings {
    /// The number the last deletion took.
    #[serde(default)]
    last: u64,
    #[serde(default)]
    pending: Vec<Pending>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    forgetting: Forgetting,
    asked_by: MachineName,
    at: Timestamp,
    /// The machine it lives on has not said yet that it deleted it.
    home: bool,
    /// The members still to say they deleted what they keep of it, once
    /// the machine it lives on did.
    #[serde(default)]
    waiting: BTreeSet<MachineName>,
    /// What each member last said it could not do yet, recorded once.
    #[serde(default)]
    failed: BTreeMap<MachineName, String>,
}

/// The member `asker` asks, as `account`, that the conversation at
/// `conversation` be deleted from every member keeping something of it.
pub(crate) fn ask(
    hub: &std::sync::Arc<Hub>,
    asker: &MachineName,
    account: AccountName,
    conversation: Address,
) -> Result<Told, String> {
    let conversation = {
        let store = hub.store();
        let directory = &store.book.directory;
        if !(directory.logged_in(asker, &account)
            || directory.same(asker, &conversation.machine) && directory.may_act_for(&account, asker))
        {
            return Err(format!("{asker} is not logged in to the account {account}"));
        }
        let home = directory
            .member(&conversation.machine)
            .filter(|member| member.is_machine())
            .ok_or_else(|| format!("there is no machine named {} on the hub", conversation.machine))?;
        Address { machine: home.name.clone(), resident: conversation.resident }
    };
    // Of a conversation its machine told is afoot, the hub knows whom it
    // works for; the machine itself is asked of any other.
    if let Some(accounts) = crate::afoot::accounts_of(hub, &conversation)
        && !accounts.contains(&account)
    {
        return Err(format!(
            "the conversation {} on {} does not work for the account {account}",
            conversation.resident, conversation.machine
        ));
    }
    let tell = {
        let mut store = hub.store();
        let by_home = store.book.directory.same(asker, &conversation.machine);
        let kept = store.book.forgetting.pending.iter().position(|pending| {
            pending.forgetting.conversation == conversation && pending.forgetting.account == account
        });
        match kept {
            // Asked again: those still to do their part are told again,
            // its machine past its part when it asks itself.
            Some(index) => {
                if by_home && store.book.forgetting.pending[index].home {
                    done_at_home(&mut store, index);
                    let forgetting = store.book.forgetting.pending[index].forgetting.clone();
                    deleted_by_asker(&mut store, asker, &forgetting)?;
                }
                told_now(&store.book.forgetting.pending[index])
            }
            None => {
                store.book.forgetting.last += 1;
                let forgetting =
                    Forgetting { id: store.book.forgetting.last, conversation: conversation.clone(), account };
                let mut pending = Pending {
                    forgetting,
                    asked_by: asker.clone(),
                    at: Timestamp::now(),
                    home: true,
                    waiting: BTreeSet::new(),
                    failed: BTreeMap::new(),
                };
                if by_home {
                    pending.home = false;
                    pending.waiting = targets(&store, &pending.forgetting);
                }
                let tell = told_now(&pending);
                let actor = Actor {
                    account: Some(pending.forgetting.account.clone()),
                    authenticated: true,
                    member: Some(asker.clone()),
                    peer: None,
                };
                let forgetting = pending.forgetting.clone();
                store.book.forgetting.pending.push(pending);
                let directory = store.book.directory.clone();
                if let Err(reason) = crate::machine_log::keep(
                    &mut store,
                    &directory,
                    actor,
                    Action::Forget { conversation: conversation.clone() },
                    Outcome::Done,
                ) {
                    store.book.forgetting.pending.pop();
                    return Err(reason);
                }
                if by_home {
                    deleted_by_asker(&mut store, asker, &forgetting)?;
                }
                // Letters held for it would only be set aside where it lived.
                let (hub, conversation) = (hub.clone(), conversation.clone());
                let held = crate::moving::Moving::hold(&hub);
                tokio::spawn(async move {
                    let _held = held;
                    match crate::mailbox::drop_for(&hub, &conversation).await {
                        Ok(0) => {}
                        Ok(dropped) => hub.log.line(format_args!(
                            "dropped {dropped} letters held for {}, being deleted",
                            conversation.key()
                        )),
                        Err(error) => hub.log.line(format_args!(
                            "dropping the letters held for {}, being deleted: {error}",
                            conversation.key()
                        )),
                    }
                });
                tell
            }
        }
    };
    hub.log.line(format_args!("{asker} asked that {} be deleted everywhere", conversation.key()));
    send(hub, tell);
    crate::afoot::settle(hub);
    Ok(Told::Done)
}

/// What the member `member` did with the deletion `id`.
pub(crate) fn forgot(hub: &Hub, member: &MachineName, id: u64, outcome: Forgot) {
    let (tell, ended) = {
        let mut store = hub.store();
        let member = store.book.directory.canonical(member).clone();
        let Some(index) = store.book.forgetting.pending.iter().position(|pending| pending.forgetting.id == id) else {
            return;
        };
        let pending = &store.book.forgetting.pending[index];
        let at_home = pending.home && store.book.directory.same(&member, &pending.forgetting.conversation.machine);
        if !at_home && (pending.home || !pending.waiting.contains(&member)) {
            // Not asked of it now: a telling it answers late.
            return;
        }
        let conversation = pending.forgetting.conversation.clone();
        let actor = Actor {
            account: Some(pending.forgetting.account.clone()),
            authenticated: true,
            member: Some(member.clone()),
            peer: None,
        };
        let action = Action::Forgotten { conversation: conversation.clone(), on: member.clone() };
        let mut tell = Vec::new();
        let (recorded, ended) = match outcome {
            Forgot::Done => {
                store.book.forgetting.pending[index].failed.remove(&member);
                if at_home {
                    done_at_home(&mut store, index);
                    tell = told_now(&store.book.forgetting.pending[index]);
                } else {
                    store.book.forgetting.pending[index].waiting.remove(&member);
                }
                // The log keeps that the machine it lived on deleted it, and
                // where it could not be; that each other machine did its part
                // as asked is the deletion going as it should, and only grew
                // the directory, by a record a machine a deletion.
                (at_home.then_some(Outcome::Done), finish(hub, &mut store, index))
            }
            // Only the machine it lives on refuses: then nothing of it is
            // deleted anywhere, and it is shown again.
            Forgot::Refused { reason } if at_home => {
                store.book.forgetting.pending.remove(index);
                (Some(Outcome::Refused { reason }), true)
            }
            Forgot::Refused { reason } | Forgot::Failed { reason } => {
                let failed = &mut store.book.forgetting.pending[index].failed;
                let new = failed.get(&member) != Some(&reason);
                failed.insert(member.clone(), reason.clone());
                (new.then_some(Outcome::Refused { reason }), false)
            }
        };
        let kept = match recorded {
            Some(outcome) => {
                let directory = store.book.directory.clone();
                crate::machine_log::keep(&mut store, &directory, actor, action, outcome)
            }
            None => store.save_book().map_err(|error| error.to_string()),
        };
        if let Err(error) = kept {
            hub.log
                .line(format_args!("keeping what {member} did with the deletion of {}: {error}", conversation.key()));
        }
        (tell, ended)
    };
    send(hub, tell);
    if ended {
        crate::afoot::settle(hub);
    }
}

/// Tells the member `name`, whose session just opened, of each deletion
/// it has a part in now.
pub(crate) fn opened(hub: &Hub, name: &MachineName) {
    let tell: Vec<(MachineName, Forgetting)> = {
        let store = hub.store();
        store
            .book
            .forgetting
            .pending
            .iter()
            .flat_map(told_now)
            .filter(|(member, _)| store.book.directory.same(member, name))
            .collect()
    };
    send(hub, tell);
}

/// Lets go of the members that left the hub: a deletion waits no longer
/// for them, and one whose conversation's machine left ends.
pub(crate) fn prune(hub: &Hub) {
    let ended = {
        let mut store = hub.store();
        let Store { book, .. } = &mut *store;
        let directory = &book.directory;
        let before = book.forgetting.pending.len();
        let mut changed = false;
        book.forgetting.pending.retain_mut(|pending| {
            let waiting = pending.waiting.len();
            pending.waiting.retain(|member| directory.member(member).is_some());
            changed |= waiting != pending.waiting.len();
            let gone = pending.home && directory.member(&pending.forgetting.conversation.machine).is_none();
            !gone && (pending.home || !pending.waiting.is_empty())
        });
        let ended = before != book.forgetting.pending.len();
        if (changed || ended)
            && let Err(error) = store.save_book()
        {
            hub.log.line(format_args!("keeping the deletions after members left: {error}"));
        }
        ended
    };
    if ended {
        crate::afoot::settle(hub);
    }
}

/// The conversations being deleted, machine by machine: no member is told
/// of them as afoot.
pub(crate) fn hidden(store: &Store) -> HashMap<MachineName, BTreeSet<ResidentId>> {
    let mut hidden: HashMap<MachineName, BTreeSet<ResidentId>> = HashMap::new();
    for pending in &store.book.forgetting.pending {
        let conversation = &pending.forgetting.conversation;
        let machine = store.book.directory.canonical(&conversation.machine).clone();
        hidden.entry(machine).or_default().insert(conversation.resident.clone());
    }
    hidden
}

/// The machine a conversation lives on asked for its deletion, having
/// deleted what it kept: the machine log says so, as of every member that
/// did its part.
fn deleted_by_asker(store: &mut Store, asker: &MachineName, forgetting: &Forgetting) -> Result<(), String> {
    let actor = Actor {
        account: Some(forgetting.account.clone()),
        authenticated: true,
        member: Some(asker.clone()),
        peer: None,
    };
    let action = Action::Forgotten { conversation: forgetting.conversation.clone(), on: asker.clone() };
    let directory = store.book.directory.clone();
    crate::machine_log::keep(store, &directory, actor, action, Outcome::Done)
}

/// Its machine deleted what it kept: every other member that may keep
/// something of it is to delete that now.
fn done_at_home(store: &mut Store, index: usize) {
    let waiting = targets(store, &store.book.forgetting.pending[index].forgetting);
    let pending = &mut store.book.forgetting.pending[index];
    pending.home = false;
    pending.waiting = waiting;
}

/// Ends the deletion at `index` when no member is left to do its part;
/// says whether it did.
fn finish(hub: &Hub, store: &mut Store, index: usize) -> bool {
    let pending = &store.book.forgetting.pending[index];
    if pending.home || !pending.waiting.is_empty() {
        return false;
    }
    let conversation = pending.forgetting.conversation.clone();
    store.book.forgetting.pending.remove(index);
    if let Err(error) = store.forget_worked(&conversation) {
        hub.log.line(format_args!("forgetting where {} worked: {error}", conversation.key()));
    }
    true
}

/// The members that may keep something of the conversation besides the
/// machine it lives on: every computer and phone logged in to the account,
/// which may have kept a copy of it while showing it; every machine the
/// account may work on, where it may have run commands; and every machine
/// the hub noted it was called to work on.
fn targets(store: &Store, forgetting: &Forgetting) -> BTreeSet<MachineName> {
    let directory = &store.book.directory;
    let (account, conversation) = (&forgetting.account, &forgetting.conversation);
    let mut targets: BTreeSet<MachineName> = directory
        .members
        .iter()
        .filter(|member| member.logins.contains(account) || directory.rights(account, &member.name).hands)
        .map(|member| member.name.clone())
        .collect();
    for (machine, worked) in &store.worked {
        if directory.same(&worked.machine, &conversation.machine)
            && worked.resident == conversation.resident
            && let Some(member) = directory.member(machine)
        {
            targets.insert(member.name.clone());
        }
    }
    targets.retain(|member| !directory.same(member, &conversation.machine));
    targets
}

/// Whom a deletion is to be told to now, and what.
fn told_now(pending: &Pending) -> Vec<(MachineName, Forgetting)> {
    if pending.home {
        vec![(pending.forgetting.conversation.machine.clone(), pending.forgetting.clone())]
    } else {
        pending.waiting.iter().map(|member| (member.clone(), pending.forgetting.clone())).collect()
    }
}

/// Tells each member connected now whose session says it forgets.
fn send(hub: &Hub, tell: Vec<(MachineName, Forgetting)>) {
    let sessions = hub.sessions();
    for (member, forgetting) in tell {
        if let Some(session) = sessions.get(&member).filter(|session| session.forgets) {
            let _ = session.out.send(Out::Frame(Down::Forget { forgetting }));
        }
    }
}
