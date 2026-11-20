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

pub(crate) mod accounts;
mod ownership;
pub(crate) mod submission;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fastctx_net::sign::PublicKey;
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
    pub(crate) last: u64,
    #[serde(default)]
    pub(crate) pending: Vec<std::sync::Arc<Pending>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    forgetting: Forgetting,
    asked_by: MachineName,
    at: Timestamp,
    /// The machine it lives on has not said yet that it deleted it.
    home: bool,
    #[serde(default)]
    ownership: Option<fastctx_types::owned_records::Conversation>,
    #[serde(default)]
    audience: BTreeSet<MachineName>,
    /// The members still to say they deleted what they keep of it, once
    /// the machine it lives on did.
    #[serde(default)]
    waiting: BTreeSet<MachineName>,
    /// What each member last said it could not do yet, recorded once.
    #[serde(default)]
    failed: BTreeMap<MachineName, String>,
    /// The device each member waited for is, by its key, as it was asked
    /// of: one that took the name since is neither told nor heard
    /// (codex-d515b194). Without one, as kept before this was, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    keys: BTreeMap<MachineName, PublicKey>,
    /// The hub deleted the conversation's scopes once the machine it lived
    /// on deleted it (`assets::deleted::conversation_gone`). Until then the
    /// deletion is kept, and that is tried again, whatever its members did:
    /// a deletion never ends with what the hub kept of it still readable.
    #[serde(default)]
    scopes_gone: bool,
}

/// The member `asker` asks, as `account`, that the conversation at
/// `conversation` be deleted from every member keeping something of it.
pub(crate) fn ask(
    hub: &std::sync::Arc<Hub>,
    asker: &MachineName,
    account: AccountName,
    conversation: Address,
) -> Result<Told, String> {
    ask_inner(hub, asker, account, conversation, false)
}

pub(crate) fn prepare(
    hub: &std::sync::Arc<Hub>,
    asker: &MachineName,
    account: AccountName,
    conversation: Address,
) -> Result<Told, String> {
    ask_inner(hub, asker, account, conversation, true)
}

fn ask_inner(
    hub: &std::sync::Arc<Hub>,
    asker: &MachineName,
    account: AccountName,
    conversation: Address,
    preparing: bool,
) -> Result<Told, String> {
    let actor = account.clone();
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
    let ownership = ownership::resolve(hub, &actor, &conversation);
    let conversation = ownership.as_ref().map_or(conversation, |owner| owner.address.clone());
    if preparing && (ownership.is_none() || !hub.store().book.directory.same(asker, &conversation.machine)) {
        return Err("only the runtime host prepares a registered conversation's deletion".into());
    }
    let account = ownership.as_ref().map_or(account, |owner| owner.account.clone());
    // Of a conversation its machine told is afoot, the hub knows whom it
    // works for; the machine itself is asked of any other.
    if ownership.is_none() && crate::afoot::owned_by(hub, &conversation, &account) == Some(false) {
        return Err(format!(
            "the conversation {} on {} does not work for the account {account}",
            conversation.resident, conversation.machine
        ));
    }
    let tell = {
        let mut store = hub.edit_store();
        if ownership.as_ref().is_some_and(|owner| !crate::owned_records::allows(&store.book.directory, &actor, owner)) {
            return Err("this account no longer owns that conversation".into());
        }
        let by_home = !preparing && store.book.directory.same(asker, &conversation.machine);
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
                let audience = ownership::targets(&store, &forgetting, ownership.as_ref());
                let mut pending = Pending {
                    forgetting,
                    asked_by: asker.clone(),
                    at: Timestamp::now(),
                    home: true,
                    ownership: ownership.clone(),
                    audience,
                    waiting: BTreeSet::new(),
                    failed: BTreeMap::new(),
                    keys: BTreeMap::new(),
                    scopes_gone: false,
                };
                if by_home {
                    pending.home = false;
                    let waiting = targets(&store, &pending);
                    wait_for(&store.book.directory, &mut pending, waiting);
                }
                let tell = told_now(&pending);
                let actor = Actor {
                    account: Some(actor.clone()),
                    authenticated: true,
                    member: Some(asker.clone()),
                    peer: None,
                };
                let forgetting = pending.forgetting.clone();
                store.book.forgetting.pending.push(std::sync::Arc::new(pending));
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
                fastctx_platform::task::spawn(async move {
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
    // Deleted where it lived already, when its own machine asks: its scopes
    // go now (`assets::deleted`).
    let gone_at_home = hub
        .store()
        .book
        .forgetting
        .pending
        .iter()
        .any(|pending| pending.forgetting.conversation == conversation && !pending.home);
    if gone_at_home {
        crate::assets::deleted::conversation_gone(hub, &conversation);
    }
    if !preparing {
        send(hub, tell);
    }
    crate::afoot::settle(hub);
    Ok(Told::Done)
}

/// The conversation at `conversation`, which worked for `account`, was
/// deleted where it lived for a purge (`crate::purges`): what other members
/// keep of it, and the hub's scopes of it, go as for a deletion its machine
/// asked for having deleted it, the hub's log saying the hub did.
pub(crate) fn gone_there(
    hub: &Hub,
    conversation: Address,
    account: AccountName,
    ownership: Option<fastctx_types::owned_records::Conversation>,
    scopes_gone: bool,
    known: BTreeSet<PublicKey>,
) -> Result<(), String> {
    let tell = {
        let mut store = hub.edit_store();
        // The machines known to keep copies of it as it was purged, which
        // the directory may no longer say, under their names now.
        let known: Vec<(MachineName, PublicKey)> = known
            .iter()
            .filter_map(|key| store.book.directory.by_key(key))
            .map(|member| (member.name.clone(), member.key))
            .collect();
        let kept = store.book.forgetting.pending.iter().position(|pending| {
            pending.forgetting.conversation == conversation && pending.forgetting.account == account
        });
        match kept {
            Some(index) => {
                // Asked by its machine already, it is told to them too
                // (codex-d515b194).
                let pending = std::sync::Arc::make_mut(&mut store.book.forgetting.pending[index]);
                let mut added = false;
                for (machine, key) in known {
                    let new = pending.audience.insert(machine.clone());
                    // Waited for already by its name alone: by its key now
                    // (codex-d515b194).
                    if !pending.home && (new || pending.waiting.contains(&machine)) {
                        added |= pending.keys.insert(machine.clone(), key) != Some(key);
                        pending.waiting.insert(machine);
                    }
                    added |= new;
                }
                let home = pending.home;
                if home {
                    done_at_home(&mut store, index);
                }
                // Not kept, the purge asks again: it does not go on as if it
                // were (codex-d515b194).
                if home || added {
                    store.save_book().map_err(|error| format!("keeping that {} went: {error}", conversation.key()))?;
                }
                told_now(&store.book.forgetting.pending[index])
            }
            None => {
                store.book.forgetting.last += 1;
                let forgetting =
                    Forgetting { id: store.book.forgetting.last, conversation: conversation.clone(), account };
                let mut audience = ownership::targets(&store, &forgetting, ownership.as_ref());
                audience.extend(known.into_iter().map(|(machine, _)| machine));
                let mut pending = Pending {
                    forgetting,
                    asked_by: conversation.machine.clone(),
                    at: Timestamp::now(),
                    home: false,
                    ownership,
                    audience,
                    waiting: BTreeSet::new(),
                    failed: BTreeMap::new(),
                    keys: BTreeMap::new(),
                    // Its purge deleted its scopes itself, of that account as
                    // opened alone (`assets::deleted::purge_scopes`).
                    scopes_gone,
                };
                let waiting = targets(&store, &pending);
                wait_for(&store.book.directory, &mut pending, waiting);
                let tell = told_now(&pending);
                store.book.forgetting.pending.push(std::sync::Arc::new(pending));
                let hub_actor = Actor { account: None, authenticated: true, member: None, peer: None };
                let action = Action::Forgotten { conversation: conversation.clone(), on: conversation.machine.clone() };
                let directory = store.book.directory.clone();
                crate::machine_log::keep(&mut store, &directory, hub_actor, action, Outcome::Done)
                    .map_err(|error| format!("keeping that {} went: {error}", conversation.key()))?;
                tell
            }
        }
    };
    if !scopes_gone {
        crate::assets::deleted::conversation_gone(hub, &conversation);
    }
    send(hub, tell);
    crate::afoot::settle(hub);
    Ok(())
}

/// What the member `member` did with the deletion `id`.
pub(crate) fn forgot(hub: &Hub, member: &MachineName, key: PublicKey, id: u64, outcome: Forgot) {
    let (tell, ended) = {
        let mut store = hub.edit_store();
        let member = store.book.directory.canonical(member).clone();
        let Some(index) = store.book.forgetting.pending.iter().position(|pending| pending.forgetting.id == id) else {
            return;
        };
        let pending = &store.book.forgetting.pending[index];
        let at_home = pending.home && store.book.directory.same(&member, &pending.forgetting.conversation.machine);
        // Waited for under the name it had as it was asked, renamed since
        // (codex-d515b194).
        let waited = pending
            .waiting
            .iter()
            .find(|waiting| is_waited(&store.book.directory, pending, waiting, &member, &key))
            .cloned();
        if !at_home && (pending.home || waited.is_none()) {
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
                std::sync::Arc::make_mut(&mut store.book.forgetting.pending[index]).failed.remove(&member);
                if at_home {
                    // Its scopes go with it (`assets::deleted`).
                    crate::assets::deleted::conversation_gone(hub, &conversation);
                    done_at_home(&mut store, index);
                    tell = told_now(&store.book.forgetting.pending[index]);
                } else if let Some(waited) = &waited {
                    let pending = std::sync::Arc::make_mut(&mut store.book.forgetting.pending[index]);
                    pending.waiting.remove(waited);
                    pending.keys.remove(waited);
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
                let failed = &mut std::sync::Arc::make_mut(&mut store.book.forgetting.pending[index]).failed;
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
    let tell: Vec<(MachineName, Option<PublicKey>, Forgetting)> = {
        let store = hub.store();
        store
            .book
            .forgetting
            .pending
            .iter()
            .flat_map(|pending| told_now(pending))
            .filter(|(member, key, _)| match key {
                Some(key) => store.book.directory.member(name).is_some_and(|found| found.key == *key),
                None => store.book.directory.same(member, name),
            })
            .collect()
    };
    send(hub, tell);
}

/// Lets go of the members that left the hub: a deletion waits no longer
/// for them, and one whose conversation's machine left ends.
pub(crate) fn prune(hub: &Hub) {
    if hub.store().book.forgetting.pending.is_empty() && hub.store().book.account_forgetting.pending.is_empty() {
        return;
    }
    let (ended, again) = {
        let mut store = hub.edit_store();
        let accounts = accounts::prune(&mut store);
        let book = &mut *store.book;
        let directory = &book.directory;
        let before = book.forgetting.pending.len();
        let mut changed = false;
        book.forgetting.pending.retain_mut(|pending| {
            let view: &Pending = pending;
            if view.waiting.iter().any(|member| !still_there(directory, view, member)) {
                let pending = std::sync::Arc::make_mut(pending);
                let view: &Pending = pending;
                let left: Vec<MachineName> =
                    view.waiting.iter().filter(|member| !still_there(directory, view, member)).cloned().collect();
                for member in left {
                    pending.waiting.remove(&member);
                    pending.keys.remove(&member);
                }
                changed = true;
            }
            let gone = pending.home && directory.member(&pending.forgetting.conversation.machine).is_none();
            !gone && (pending.home || !pending.waiting.is_empty() || !pending.scopes_gone)
        });
        let ended = before != book.forgetting.pending.len();
        // Deleted where it lived, its scopes not yet: tried again
        // (`assets::deleted` spaces the tries).
        let again: Vec<Address> = book
            .forgetting
            .pending
            .iter()
            .filter(|pending| !pending.home && !pending.scopes_gone)
            .map(|pending| pending.forgetting.conversation.clone())
            .collect();
        if (changed || ended || accounts)
            && let Err(error) = store.save_book()
        {
            hub.log.line(format_args!("keeping the deletions after members left: {error}"));
        }
        (ended, again)
    };
    for conversation in &again {
        crate::assets::deleted::conversation_gone(hub, conversation);
    }
    if ended {
        crate::afoot::settle(hub);
    }
}

/// The hub deleted the scopes of `conversation`, deleted where it lived:
/// each deletion of it waits for that no more, and ends when no member is
/// left to do its part.
pub(crate) fn scopes_went(hub: &Hub, conversation: &Address) {
    let ended = {
        let mut store = hub.edit_store();
        let at: Vec<usize> = store
            .book
            .forgetting
            .pending
            .iter()
            .enumerate()
            .filter(|(_, pending)| {
                !pending.home && !pending.scopes_gone && pending.forgetting.conversation == *conversation
            })
            .map(|(index, _)| index)
            .collect();
        if at.is_empty() {
            return;
        }
        let mut ended = false;
        for &index in at.iter().rev() {
            std::sync::Arc::make_mut(&mut store.book.forgetting.pending[index]).scopes_gone = true;
            ended |= finish(hub, &mut store, index);
        }
        if let Err(error) = store.save_book() {
            hub.log.line(format_args!("keeping that the scopes of {} went: {error}", conversation.key()));
        }
        ended
    };
    if ended {
        crate::afoot::settle(hub);
    }
}

/// The conversations being deleted, machine by machine: no member is told
/// of them as afoot.
pub(crate) fn hidden(store: &crate::store::Snapshot) -> HashMap<MachineName, BTreeSet<ResidentId>> {
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
    let waiting = targets(store, &store.book.forgetting.pending[index]);
    let directory = store.book.directory.clone();
    let pending = std::sync::Arc::make_mut(&mut store.book.forgetting.pending[index]);
    pending.home = false;
    wait_for(&directory, pending, waiting);
}

/// Sets whom `pending` waits for: `waiting`, each with the key of the member
/// that has the name now (`Pending::keys`).
fn wait_for(directory: &fastctx_net::directory::Directory, pending: &mut Pending, waiting: BTreeSet<MachineName>) {
    pending.keys =
        waiting.iter().filter_map(|name| directory.member(name).map(|member| (name.clone(), member.key))).collect();
    pending.waiting = waiting;
}

/// Whether `waited`, as `pending` waits for it, is the member named
/// `member` now, whose session proved `key`: by the key it waits for where
/// it has one (codex-d515b194).
fn is_waited(
    directory: &fastctx_net::directory::Directory,
    pending: &Pending,
    waited: &MachineName,
    member: &MachineName,
    key: &PublicKey,
) -> bool {
    match pending.keys.get(waited) {
        Some(waited) => waited == key,
        None => directory.same(waited, member),
    }
}

/// Whether the member `pending` waits for as `waited` is in the hub still.
fn still_there(directory: &fastctx_net::directory::Directory, pending: &Pending, waited: &MachineName) -> bool {
    match pending.keys.get(waited) {
        Some(key) => directory.by_key(key).is_some(),
        None => directory.member(waited).is_some(),
    }
}

/// Ends the deletion at `index` when no member is left to do its part;
/// says whether it did.
fn finish(hub: &Hub, store: &mut Store, index: usize) -> bool {
    let pending = &store.book.forgetting.pending[index];
    if pending.home || !pending.waiting.is_empty() || !pending.scopes_gone {
        return false;
    }
    let conversation = pending.forgetting.conversation.clone();
    store.book.forgetting.pending.remove(index);
    if let Err(error) = store.forget_worked(&conversation) {
        hub.log.line(format_args!("forgetting where {} worked: {error}", conversation.key()));
    }
    true
}

/// Current and former asset readers, plus machines with recorded work, may
/// need cleanup. Administrative machine access alone is not a recipient.
fn targets(store: &crate::store::Snapshot, pending: &Pending) -> BTreeSet<MachineName> {
    let directory = &store.book.directory;
    let mut targets = ownership::targets(store, &pending.forgetting, pending.ownership.as_ref());
    // Under their names now (codex-d515b194).
    targets.extend(
        pending.audience.iter().filter_map(|machine| directory.member(machine)).map(|member| member.name.clone()),
    );
    targets
}

/// Whom a deletion is to be told to now, and what.
fn told_now(pending: &Pending) -> Vec<(MachineName, Option<PublicKey>, Forgetting)> {
    if pending.home {
        vec![(pending.forgetting.conversation.machine.clone(), None, pending.forgetting.clone())]
    } else {
        pending
            .waiting
            .iter()
            .map(|member| (member.clone(), pending.keys.get(member).copied(), pending.forgetting.clone()))
            .collect()
    }
}

/// Tells each member connected now whose session says it forgets.
fn send(hub: &Hub, tell: Vec<(MachineName, Option<PublicKey>, Forgetting)>) {
    if tell.is_empty() {
        return;
    }
    let sessions = hub.sessions();
    let store = hub.store();
    for (member, key, forgetting) in tell {
        // The device it was asked of, under its name now: one that took the
        // name since is not it (codex-d515b194).
        let directory = &store.book.directory;
        let found = match key {
            Some(key) => directory.by_key(&key).map(|found| &found.name),
            None => Some(directory.canonical(&member)),
        };
        // And its session that device's own (codex-d515b194).
        if let Some(session) = found
            .and_then(|name| sessions.get(name))
            .filter(|session| session.forgets && key.is_none_or(|key| session.key == key))
        {
            let _ = session.out.send(Out::Frame(Down::Forget { forgetting }));
        }
    }
}
