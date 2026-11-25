//! Metadata-only ownership index, rebuilt from committed account records.

use crate::assets::bodies::Bodies;
use fastctx_net::{authorization::Lookup, directory::Directory};
use fastctx_types::{AccountName, Address, MachineName, owned_records::Conversation};
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// A conversation by the account that opened it, as opened, and its key.
pub(crate) type Identity = (AccountName, Timestamp, String);

#[derive(Default)]
pub(crate) struct Index {
    conversations: BTreeMap<Identity, Conversation>,
    addresses: BTreeMap<(MachineName, fastctx_types::ResidentId), BTreeSet<Identity>>,
    accounts: BTreeMap<AccountName, BTreeSet<Identity>>,
    machines: BTreeMap<MachineName, BTreeSet<Identity>>,
    /// The hub's bodies, which know the conversations described into one.
    bodies: Option<Arc<Bodies>>,
}

impl Index {
    /// Takes the hub's bodies, once the asset library is open.
    pub(crate) fn attach(&mut self, bodies: Arc<Bodies>) {
        self.bodies = Some(bodies);
    }

    /// The accounts, as opened, that conversations described into a body
    /// keep their records with, removed or not (§29, 2.6).
    pub(crate) fn body_openers(&self) -> std::collections::BTreeSet<(fastctx_types::AccountName, jiff::Timestamp)> {
        self.bodies.as_ref().map(|bodies| bodies.conversation_openers()).unwrap_or_default()
    }

    /// The body the conversation `identity` was described into.
    fn body(&self, identity: &Identity) -> Option<String> {
        self.bodies.as_ref()?.conversation(identity)
    }

    /// Whether the conversation `identity` still has readers to answer: its
    /// opener is the account it was, or it is a body's, whose records stay
    /// its readers' with the opener gone (§29, 2.6).
    pub(crate) fn live<L: Lookup + ?Sized>(&self, directory: &L, identity: &Identity) -> bool {
        directory.find_account(&identity.0).is_some_and(|owner| owner.opened == identity.1)
            || self.body(identity).is_some()
    }

    /// Whether `actor` reads and writes the records of the conversation
    /// `identity`: any by its opener, or those its birth machine is held by,
    /// while its opener is the account it was; a body's by the body's
    /// members too, its opener gone or not (§29, 2.6).
    pub(crate) fn reads(
        &self,
        directory: &Directory,
        actor: &AccountName,
        identity: &Identity,
        conversation: &Conversation,
    ) -> bool {
        let opener = directory.account(&identity.0).is_some_and(|owner| owner.opened == identity.1);
        if opener && super::allows(directory, actor, conversation) {
            return true;
        }
        let (Some(body), Some(bodies)) = (self.body(identity), self.bodies.as_ref()) else { return false };
        bodies.member(directory, actor, &body)
    }

    pub(crate) fn of_record(
        &self,
        account: &AccountName,
        opened: Timestamp,
        kind: &str,
        key: &str,
    ) -> Option<&Conversation> {
        let resident = match kind {
            fastctx_types::owned_records::KIND | fastctx_types::plan::KIND | "goal" | "goal_past" => key,
            fastctx_types::history::RECORD_KIND => key.split_once('/')?.0,
            _ => return None,
        };
        self.conversations.get(&(account.clone(), opened, resident.into()))
    }

    pub(crate) fn take(&mut self, account: &AccountName, opened: Timestamp, key: &str, body: Option<Conversation>) {
        let identity = (account.clone(), opened, key.to_string());
        if let Some(previous) = self.conversations.remove(&identity) {
            for machine in std::iter::once(&previous.address.machine).chain(&previous.previous) {
                if let Some(keys) = self.addresses.get_mut(&(machine.clone(), previous.address.resident.clone())) {
                    keys.remove(&identity);
                }
            }
            if let Some(keys) = self.accounts.get_mut(account) {
                keys.remove(&identity);
            }
            if let Some(keys) = previous.shared_on.as_ref().and_then(|machine| self.machines.get_mut(machine)) {
                keys.remove(&identity);
            }
        }
        if let Some(body) = body {
            for machine in std::iter::once(&body.address.machine).chain(&body.previous) {
                self.addresses
                    .entry((machine.clone(), body.address.resident.clone()))
                    .or_default()
                    .insert(identity.clone());
            }
            self.accounts.entry(account.clone()).or_default().insert(identity.clone());
            if let Some(machine) = &body.shared_on {
                self.machines.entry(machine.clone()).or_default().insert(identity.clone());
            }
            self.conversations.insert(identity, body);
        }
    }

    /// The conversations whose records `account` reads.
    pub(crate) fn readable(&self, directory: &Directory, account: &AccountName) -> Vec<Conversation> {
        self.readable_at(directory, account).into_iter().map(|(conversation, _)| conversation).collect()
    }

    /// The same, each with when its opener's account was opened as it opened
    /// it: a body's outlives the opener, and one made again under the name
    /// is another (§29, 2.6).
    pub(crate) fn readable_at(&self, directory: &Directory, account: &AccountName) -> Vec<(Conversation, Timestamp)> {
        let mut keys = self.accounts.get(account).cloned().unwrap_or_default();
        // A body's conversations, its machine gone from the directory or not.
        if let Some(bodies) = &self.bodies {
            let mut member: BTreeMap<String, bool> = BTreeMap::new();
            for (identity, body) in bodies.conversations() {
                let is = *member.entry(body.clone()).or_insert_with(|| bodies.member(directory, account, &body));
                if is && self.conversations.contains_key(&identity) {
                    keys.insert(identity);
                }
            }
        }
        let machines: BTreeSet<_> = directory
            .members
            .iter()
            .filter(|member| member.home.as_ref() == Some(account))
            .map(|member| member.name.clone())
            .chain(directory.shares.iter().filter(|share| &share.account == account).map(|share| share.machine.clone()))
            .collect();
        for name in machines {
            if !directory.holds(account, &name) {
                continue;
            }
            let Some(machine) = directory.member(&name) else { continue };
            for name in std::iter::once(&machine.name).chain(&machine.was) {
                if let Some(shared) = self.machines.get(name) {
                    keys.extend(shared.iter().cloned());
                }
            }
        }
        keys.into_iter()
            .filter_map(|identity| {
                let conversation = self.conversations.get(&identity)?;
                (self.live(directory, &identity) && self.reads(directory, account, &identity, conversation))
                    .then(|| (super::verified(directory, conversation), identity.1))
            })
            .collect()
    }

    /// The one conversation registered at `address`, whoever owns it.
    pub(crate) fn at(&self, directory: &fastctx_net::directory::Directory, address: &Address) -> Option<&Conversation> {
        let names: Vec<_> = match directory.member(&address.machine) {
            Some(member) => std::iter::once(&member.name).chain(&member.was).collect(),
            None => vec![&address.machine],
        };
        let mut found = names
            .into_iter()
            .filter_map(|name| self.addresses.get(&(name.clone(), address.resident.clone())))
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|identity| self.conversations.get(identity));
        let conversation = found.next()?;
        found.next().is_none().then_some(conversation)
    }

    /// Whether an address belongs to a current account, regardless of the reader's access.
    pub(crate) fn registered_at<L: fastctx_net::authorization::Lookup + ?Sized>(
        &self,
        directory: &L,
        address: &Address,
    ) -> bool {
        let names: Vec<_> = match directory.find_member(&address.machine) {
            Some(member) => std::iter::once(&member.name).chain(&member.was).collect(),
            None => vec![&address.machine],
        };
        names.into_iter().any(|name| {
            self.addresses
                .get(&(name.clone(), address.resident.clone()))
                .is_some_and(|keys| keys.iter().any(|identity| self.live(directory, identity)))
        })
    }

    /// The conversation at `address` whose records `account` reads, and
    /// when its opener's account was opened.
    pub(crate) fn resolve(
        &self,
        directory: &Directory,
        account: &AccountName,
        address: &Address,
    ) -> Result<(Conversation, Timestamp), String> {
        let member = directory.find_member(&address.machine);
        let names: Vec<_> = match member {
            Some(member) => std::iter::once(&member.name).chain(&member.was).collect(),
            None => vec![&address.machine],
        };
        let mut candidates = BTreeSet::new();
        for name in names {
            if let Some(keys) = self.addresses.get(&(name.clone(), address.resident.clone())) {
                candidates.extend(keys.iter().cloned());
            }
        }
        let mut found = candidates.into_iter().filter_map(|identity| {
            if !self.live(directory, &identity) {
                return None;
            }
            let conversation = self.conversations.get(&identity)?;
            self.reads(directory, account, &identity, conversation)
                .then(|| (super::verified(directory, conversation), identity.1))
        });
        let conversation = found.next().ok_or("this account has no record of that conversation")?;
        if found.next().is_some() {
            return Err("the conversation address identifies more than one account record".into());
        }
        Ok(conversation)
    }

    /// The conversation `resident` that `account`, as opened, opened, and
    /// whether it was described into a body (`crate::purges`).
    pub(crate) fn opened(
        &self,
        account: &AccountName,
        opened: Timestamp,
        resident: &str,
    ) -> Option<(&Conversation, bool)> {
        let identity = (account.clone(), opened, resident.to_string());
        let conversation = self.conversations.get(&identity)?;
        Some((conversation, self.body(&identity).is_some()))
    }

    /// The conversations `account`, as opened, opened that say they were
    /// born under any of `grants`, found by the hub or still to be found
    /// (`crate::purges`).
    pub(crate) fn born_under(
        &self,
        account: &AccountName,
        opened: Timestamp,
        grants: &[fastctx_types::access::GrantId],
    ) -> Vec<&Conversation> {
        self.accounts
            .get(account)
            .into_iter()
            .flatten()
            .filter(|identity| identity.1 == opened)
            .filter_map(|identity| self.conversations.get(identity))
            .filter(|conversation| {
                [&conversation.birth.born_under, &conversation.birth.claimed_under]
                    .into_iter()
                    .flatten()
                    .any(|grant| grants.contains(grant))
            })
            .collect()
    }

    /// Whether `machine`, by any name it had, registered a conversation.
    pub(crate) fn registered(&self, directory: &fastctx_net::directory::Directory, machine: &MachineName) -> bool {
        let names: Vec<_> = match directory.member(machine) {
            Some(member) => std::iter::once(&member.name).chain(&member.was).collect(),
            None => vec![machine],
        };
        self.addresses.keys().any(|(name, _)| names.contains(&name))
    }
}
