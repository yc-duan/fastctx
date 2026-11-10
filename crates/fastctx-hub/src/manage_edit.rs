//! Management edits whose authority inputs and row identities stay unchanged.

use fastctx_net::wire::Change;
use fastctx_types::AccountName;

use crate::store::{BookEdit, Store};

#[cfg(feature = "change-drill")]
std::thread_local! {
    static FORCE_FULL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(feature = "change-drill")]
pub(crate) fn forced_full<T>(work: impl FnOnce() -> T) -> T {
    FORCE_FULL.with(|force| {
        let before = force.replace(true);
        let result = work();
        force.set(before);
        result
    })
}

pub(crate) fn rows(store: &Store, account: &AccountName, change: &Change) -> BookEdit {
    #[cfg(feature = "change-drill")]
    if FORCE_FULL.with(std::cell::Cell::get) {
        return BookEdit::Full;
    }
    if !store.acting_clean {
        return BookEdit::Full;
    }
    let index = store.indexed();
    match change {
        Change::History { .. }
        | Change::Collab { .. }
        | Change::Profile { .. }
        | Change::MemorySettings { .. }
        | Change::Layer { .. }
        | Change::Subagents { .. } => {
            index.accounts_named(account).first().copied().map_or(BookEdit::Audit, BookEdit::Account)
        }
        Change::Group { machine, .. } => index
            .member(machine)
            .and_then(|member| index.key_position(&member.key))
            .map_or(BookEdit::Audit, |at| BookEdit::Groups(vec![at])),
        Change::RenameGroup { owner, from, .. } => BookEdit::Groups(
            index
                .owned(owner)
                .iter()
                .copied()
                .filter(|&at| index.all.members[at].group.as_deref() == Some(from.as_str()))
                .collect(),
        ),
        Change::RevokeToken { .. } => BookEdit::Tokens,
        Change::Settings { .. } | Change::Updates { .. } | Change::Rollout { .. } | Change::Intake { .. } => {
            BookEdit::Audit
        }
        _ => BookEdit::Full,
    }
}

/// Changes that cannot revoke a running call, transfer, machine or account.
pub(crate) fn configuration(change: &Change) -> bool {
    #[cfg(feature = "change-drill")]
    if FORCE_FULL.with(std::cell::Cell::get) {
        return false;
    }
    matches!(
        change,
        Change::History { .. }
            | Change::Collab { .. }
            | Change::Profile { .. }
            | Change::MemorySettings { .. }
            | Change::Layer { .. }
            | Change::Subagents { .. }
            | Change::Group { .. }
            | Change::RenameGroup { .. }
            | Change::RevokeToken { .. }
            | Change::Settings { .. }
            | Change::Updates { .. }
            | Change::Rollout { .. }
            | Change::Intake { .. }
    )
}
