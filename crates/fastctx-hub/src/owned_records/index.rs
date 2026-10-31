//! Metadata-only ownership index, rebuilt from committed account records.

use fastctx_types::{AccountName, Address, MachineName, owned_records::Conversation};
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

type Identity = (AccountName, Timestamp, String);

#[derive(Default)]
pub(crate) struct Index {
    conversations: BTreeMap<Identity, Conversation>,
    addresses: BTreeMap<(MachineName, fastctx_types::ResidentId), BTreeSet<Identity>>,
    accounts: BTreeMap<AccountName, BTreeSet<Identity>>,
    machines: BTreeMap<MachineName, BTreeSet<Identity>>,
}

impl Index {
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

    pub(crate) fn candidates(
        &self,
        directory: &fastctx_net::directory::Directory,
        account: &AccountName,
    ) -> Vec<Conversation> {
        let mut keys = self.accounts.get(account).cloned().unwrap_or_default();
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
                if directory.account(&identity.0).is_none_or(|account| account.opened != identity.1) {
                    return None;
                }
                self.conversations.get(&identity).cloned()
            })
            .collect()
    }

    pub(crate) fn resolve(
        &self,
        directory: &fastctx_net::directory::Directory,
        account: &AccountName,
        address: &Address,
    ) -> Result<Conversation, String> {
        let member = directory.member(&address.machine);
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
            if directory.account(&identity.0).is_none_or(|owner| owner.opened != identity.1) {
                return None;
            }
            let conversation = self.conversations.get(&identity)?;
            super::allows(directory, account, conversation).then(|| conversation.clone())
        });
        let conversation = found.next().ok_or("this account has no record of that conversation")?;
        if found.next().is_some() {
            return Err("the conversation address identifies more than one account record".into());
        }
        Ok(conversation)
    }
}
