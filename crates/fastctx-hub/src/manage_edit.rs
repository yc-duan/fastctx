//! Management edits whose authority inputs and row identities stay unchanged.

use fastctx_net::wire::Change;
use fastctx_types::AccountName;

use crate::store::{BookEdit, Store};

pub(crate) fn rows(store: &Store, account: &AccountName, change: &Change) -> BookEdit {
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
        Change::Token { .. } | Change::RevokeToken { .. } => BookEdit::Tokens,
        Change::Settings { .. }
        | Change::Updates { .. }
        | Change::Rollout { .. }
        | Change::Intake { .. }
        | Change::UpdateDue { .. } => BookEdit::Audit,
        _ => BookEdit::Full,
    }
}

/// Changes that cannot revoke a running call, transfer, machine or account.
pub(crate) fn configuration(change: &Change) -> bool {
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
            | Change::Plan { .. }
            | Change::Token { .. }
            | Change::RevokeToken { .. }
            | Change::PasswordStart { .. }
            | Change::PasswordFinish { .. }
            | Change::Settings { .. }
            | Change::Updates { .. }
            | Change::Rollout { .. }
            | Change::Intake { .. }
            | Change::UpdateDue { .. }
    )
}
