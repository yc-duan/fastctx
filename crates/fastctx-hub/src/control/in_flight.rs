//! The calls on their way through the hub, counted by their member and
//! their account as they come and go, and found by their caller's id: how
//! many a member or an account has, or which one a caller gives up, is
//! told at once however many there are (`world-concurrency.md`, 规矩不随
//! 规模变坏).

use std::collections::HashMap;
use std::sync::LazyLock;

use fastctx_types::{AccountName, MachineName};

use super::Pending;

#[derive(Default)]
pub(crate) struct InFlight {
    calls: HashMap<u64, Pending>,
    members: HashMap<MachineName, usize>,
    accounts: HashMap<AccountName, usize>,
    /// The hub's key of a call by its caller, the caller's session and the
    /// caller's own id for it.
    asked: HashMap<(MachineName, u64, u64), u64>,
}

/// The most calls one member, and one account across its machines, may
/// have on their way through the hub at once. Each holds a few hundred
/// bytes here, so these follow the hub's memory and only stop a member gone
/// wrong; the others are forwarded as before.
pub(super) fn shares() -> (usize, usize) {
    static SHARES: LazyLock<(usize, usize)> = LazyLock::new(|| {
        let member =
            (fastctx_platform::memory::physical().unwrap_or(1 << 30) / (128 << 10)).clamp(4096, 65_536) as usize;
        (member, member * 4)
    });
    *SHARES
}

fn less<K: Eq + std::hash::Hash>(counts: &mut HashMap<K, usize>, key: &K) {
    if let Some(n) = counts.get_mut(key) {
        *n -= 1;
        if *n == 0 {
            counts.remove(key);
        }
    }
}

impl InFlight {
    pub(crate) fn insert(&mut self, key: u64, pending: Pending) {
        if let Some(replaced) = self.calls.remove(&key) {
            self.forget(key, &replaced);
        }
        *self.members.entry(pending.caller.clone()).or_default() += 1;
        *self.accounts.entry(pending.standing.account.clone()).or_default() += 1;
        self.asked.insert((pending.caller.clone(), pending.caller_generation, pending.id), key);
        self.calls.insert(key, pending);
    }

    pub(crate) fn remove(&mut self, key: &u64) -> Option<Pending> {
        let pending = self.calls.remove(key)?;
        self.forget(*key, &pending);
        Some(pending)
    }

    fn forget(&mut self, key: u64, pending: &Pending) {
        less(&mut self.members, &pending.caller);
        less(&mut self.accounts, &pending.standing.account);
        let asked = (pending.caller.clone(), pending.caller_generation, pending.id);
        if self.asked.get(&asked) == Some(&key) {
            self.asked.remove(&asked);
        }
    }

    pub(crate) fn get(&self, key: &u64) -> Option<&Pending> {
        self.calls.get(key)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&u64, &Pending)> {
        self.calls.iter()
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &Pending> {
        self.calls.values()
    }

    pub(crate) fn len(&self) -> usize {
        self.calls.len()
    }

    /// The hub's key of the call `id` that `caller` made in its session
    /// `generation`.
    pub(crate) fn asked_by(&self, caller: &MachineName, generation: u64, id: u64) -> Option<u64> {
        self.asked.get(&(caller.clone(), generation, id)).copied()
    }

    pub(crate) fn of_member(&self, member: &MachineName) -> usize {
        self.members.get(member).copied().unwrap_or(0)
    }

    pub(crate) fn of_account(&self, account: &AccountName) -> usize {
        self.accounts.get(account).copied().unwrap_or(0)
    }
}
