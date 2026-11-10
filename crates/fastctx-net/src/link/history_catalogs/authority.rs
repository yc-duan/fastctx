//! Reuse indexed account permissions across catalogs and readers of one view.

use super::Key;
use crate::{
    authorization::Action,
    directory::View,
    directory_index::{Index, Indexed},
};
use fastctx_types::{AccountName, owned_records::Conversation};
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

#[derive(Default)]
pub(super) struct Cache {
    view: Weak<View>,
    index: Index,
    accounts: BTreeMap<AccountName, Arc<Vec<u8>>>,
}
#[derive(PartialEq, Eq)]
pub(super) struct Fingerprint {
    account: Arc<Vec<u8>>,
    scope: Vec<u8>,
}
impl Cache {
    pub(super) fn get(
        &mut self,
        view: &Arc<View>,
        (account, source): &Key,
        owner: Option<&Conversation>,
    ) -> Fingerprint {
        if !self.view.upgrade().is_some_and(|before| Arc::ptr_eq(&before, view)) {
            self.view = Arc::downgrade(view);
            self.index = Index::new(view);
            self.accounts.clear();
        }
        let indexed = Indexed::new(view, &self.index);
        let common = self
            .accounts
            .entry(account.clone())
            .or_insert_with(|| {
                // A formerly hidden author can become visible after a work grant.
                let hands: Vec<_> = view
                    .machines()
                    .filter(|member| indexed.authorize(account, Action::Work(&member.name)).is_ok())
                    .map(|member| (&member.name, &member.was))
                    .collect();
                Arc::new(
                    serde_json::to_vec(&(
                        view.shared_records,
                        &view.me,
                        indexed.member(&view.me).map(|member| &member.key),
                        indexed.account(account).map(|account| (account.locked, account.opened)),
                        hands,
                    ))
                    .expect("catalog account authority serializes"),
                )
            })
            .clone();
        let project = match source {
            super::Source::Project(town) => {
                Some((view.project(town), indexed.authorize(account, Action::Project(town)).is_ok()))
            }
            _ => None,
        };
        let ownership = owner.map(|owner| {
            (
                indexed.account(&owner.account).map(|account| (account.locked, account.opened)),
                owner.shared_on.as_ref().map(|machine| {
                    (indexed.holds(account, machine), indexed.member(machine).and_then(crate::record_ownership::seal))
                }),
            )
        });
        Fingerprint {
            account: common,
            scope: serde_json::to_vec(&(indexed.may_act_for(account, &view.me), project, ownership))
                .expect("catalog scope authority serializes"),
        }
    }
}
