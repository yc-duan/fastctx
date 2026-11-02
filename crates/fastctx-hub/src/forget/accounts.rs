//! Letting members keep nothing of an account deleted (`world-engine.md`,
//! 对话作为一个整体): the machines that may keep something done for it and
//! stay on the hub, those logged in to it, shared with it or assisting it
//! when it went, let go of what they keep of it. Its own machines leave the
//! hub with it and keep what is theirs, as on a network dissolved. The hub
//! keeps each deletion until each of them says it is done, telling each now
//! when it is connected and again at every session it opens, as it does a
//! conversation's (`super`).

use std::collections::BTreeSet;

use fastctx_net::wire::{AccountForgetting, Down, Forgot};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::store::Store;
use crate::{Hub, Out};

/// The account deletions the hub keeps, with the directory (`crate::store`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct AccountForgettings {
    /// The number the last deletion took.
    #[serde(default)]
    pub(crate) last: u64,
    #[serde(default)]
    pub(crate) pending: Vec<std::sync::Arc<AccountPending>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AccountPending {
    pub(crate) forgetting: AccountForgetting,
    /// The members still to say they let go of what they keep of it.
    pub(crate) waiting: BTreeSet<MachineName>,
}

/// The machines that may keep something done for `account` and stay on the
/// hub when it goes, read before it does.
pub(crate) fn audience(store: &Store, account: &AccountName) -> BTreeSet<MachineName> {
    let directory = &store.book.directory;
    let mut machines: BTreeSet<MachineName> = directory
        .members
        .iter()
        .filter(|member| member.is_machine() && member.logins.contains(account))
        .map(|member| member.name.clone())
        .collect();
    machines
        .extend(directory.shares.iter().filter(|share| share.account == *account).map(|share| share.machine.clone()));
    machines.extend(
        directory.assists.iter().filter(|assist| assist.account == *account).map(|assist| assist.machine.clone()),
    );
    machines.retain(|machine| {
        directory.member(machine).is_some_and(|member| member.is_machine() && member.home.as_ref() != Some(account))
    });
    machines
}

/// Keeps the deletion of `account` for the machines `waiting`, numbered.
pub(crate) fn keep(store: &mut Store, account: AccountName, waiting: BTreeSet<MachineName>) {
    if waiting.is_empty() {
        return;
    }
    let deletions = &mut store.book.account_forgetting;
    deletions.last += 1;
    let forgetting = AccountForgetting { id: deletions.last, account, at: Timestamp::now() };
    deletions.pending.push(std::sync::Arc::new(AccountPending { forgetting, waiting }));
}

/// Tells each member connected now what it is still to let go of, where
/// its session says it forgets accounts; of `member` only, when given.
pub(crate) fn tell(hub: &Hub, member: Option<&MachineName>) {
    let tell: Vec<(MachineName, AccountForgetting)> = {
        let store = hub.store();
        let directory = &store.book.directory;
        store
            .book
            .account_forgetting
            .pending
            .iter()
            .flat_map(|pending| {
                pending
                    .waiting
                    .iter()
                    .filter(|waiting| member.is_none_or(|member| directory.same(waiting, member)))
                    .map(|waiting| (waiting.clone(), pending.forgetting.clone()))
            })
            .collect()
    };
    if tell.is_empty() {
        return;
    }
    let sessions = hub.sessions();
    for (member, forgetting) in tell {
        if let Some(session) = sessions.get(&member).filter(|session| session.forgets_accounts) {
            let _ = session.out.send(Out::Frame(Down::ForgetAccount { forgetting }));
        }
    }
}

/// What the member `member` did with the account deletion `id`: one done
/// waits no longer for it; one it could not do yet is told again at its next
/// session.
pub(crate) fn forgot(hub: &Hub, member: &MachineName, id: u64, outcome: Forgot) {
    let mut store = hub.edit_store();
    let member = store.book.directory.canonical(member).clone();
    let deletions = &mut store.book.account_forgetting;
    let Some(index) = deletions.pending.iter().position(|pending| pending.forgetting.id == id) else { return };
    match outcome {
        Forgot::Done => {
            let pending = std::sync::Arc::make_mut(&mut deletions.pending[index]);
            if !pending.waiting.remove(&member) {
                return;
            }
            if pending.waiting.is_empty() {
                deletions.pending.remove(index);
            }
            if let Err(error) = store.save_book() {
                hub.log.line(format_args!("keeping that {member} let go of a deleted account: {error}"));
            }
        }
        Forgot::Refused { reason } | Forgot::Failed { reason } => {
            let account = &deletions.pending[index].forgetting.account;
            hub.log.line(format_args!("{member} could not yet let go of the deleted account {account}: {reason}"));
        }
    }
}

/// Lets go of the members that left the hub: a deletion waits no longer for
/// them. Says whether any was let go of.
pub(crate) fn prune(store: &mut Store) -> bool {
    let book = &mut *store.book;
    let directory = &book.directory;
    let before = book.account_forgetting.pending.len();
    let mut changed = false;
    book.account_forgetting.pending.retain_mut(|pending| {
        if pending.waiting.iter().any(|member| directory.member(member).is_none()) {
            std::sync::Arc::make_mut(pending).waiting.retain(|member| directory.member(member).is_some());
            changed = true;
        }
        !pending.waiting.is_empty()
    });
    changed || before != book.account_forgetting.pending.len()
}
