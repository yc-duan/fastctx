//! Quota readings belong to an identified account, never just its directory.
use crate::Node;
use fastctx_types::logins::Identity;
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) fn key(identity: &Identity, anonymous_generation: &impl serde::Serialize) -> String {
    let named = identity.email.is_some() || identity.organization.is_some();
    let bytes = serde_json::to_vec(&(
        identity.logged_in,
        &identity.method,
        &identity.email,
        &identity.organization,
        (!named).then_some(anonymous_generation),
    ))
    .expect("account identity serializes");
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn select(node: &Node, agent: &str, dir: &Path, account: String, at: Timestamp) -> std::io::Result<()> {
    let path = super::ledger(node, agent, dir);
    super::with_book(node, &path, |book| {
        if book.accounts.last().is_some_and(|(since, old)| *since > at || *old == account) {
            return Ok(());
        }
        book.commit(&path, super::Entry::Account { at, account })
    })
}

pub(crate) fn at(node: &Node, agent: &str, dir: &Path, at: Timestamp) -> std::io::Result<Option<String>> {
    super::with_book(node, &super::ledger(node, agent, dir), |book| Ok(book.account_at(at)))
}

pub(crate) fn first(node: &Node, agent: &str, dir: &Path) -> std::io::Result<Option<String>> {
    super::with_book(node, &super::ledger(node, agent, dir), |book| {
        Ok(book.accounts.first().map(|(_, key)| key.clone()))
    })
}
