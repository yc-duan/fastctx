//! Captured record ownership, rechecked against the current directory at delivery.

use fastctx_net::{authorization::Action, directory_index::Indexed, sign::PublicKey};
use fastctx_types::{AccountName, MachineName, account_records::Metadata, owned_records::Conversation};
use jiff::Timestamp;

pub(crate) struct Hint {
    account: AccountName,
    conversation: Option<(Timestamp, Conversation)>,
    pub(crate) replica: bool,
}

impl Hint {
    pub(crate) fn new(
        hub: &crate::Hub,
        directory: &Indexed<'_>,
        from: &MachineName,
        key: &PublicKey,
        account: &AccountName,
        metadata: &Metadata,
    ) -> Self {
        let replica = crate::record_subscriptions::replica_reader(hub, from, key);
        let conversation = replica
            .then(|| {
                let opened = directory.account(account)?.opened;
                let owners = hub.account_records.owners.lock().expect("never poisoned");
                owners.of_record(account, opened, &metadata.kind, &metadata.key).cloned().map(|owner| (opened, owner))
            })
            .flatten();
        Self { account: account.clone(), conversation, replica }
    }

    pub(crate) fn allows(&self, directory: &Indexed<'_>, from: &MachineName, key: &PublicKey) -> bool {
        let Some(member) = directory.member(from).filter(|member| member.key == *key) else { return false };
        if directory.authorize(&self.account, Action::ActFor(from)).is_ok() {
            return true;
        }
        let Some((opened, conversation)) = &self.conversation else { return false };
        directory.account(&self.account).is_some_and(|owner| owner.opened == *opened)
            && member.logins.iter().any(|actor| super::allows(directory, actor, conversation))
    }
}

pub(crate) fn sees_writer(directory: &Indexed<'_>, from: &MachineName, by: &MachineName) -> bool {
    directory.member(from).is_some_and(|reader| {
        reader.logins.iter().any(|account| directory.authorize(account, Action::Work(by)).is_ok())
    })
}
