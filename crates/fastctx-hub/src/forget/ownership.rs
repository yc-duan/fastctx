//! A deletion retains its asset ownership and former addresses before the source is erased.
use super::*;
use fastctx_types::owned_records::Conversation;

pub(super) fn resolve(hub: &Hub, actor: &AccountName, address: &Address) -> Option<Conversation> {
    let store = hub.store();
    let directory = &store.book.directory;
    if let Ok(conversation) =
        hub.account_records.owners.lock().expect("never poisoned").resolve(directory, actor, address)
    {
        return Some(conversation);
    }
    store
        .book
        .forgetting
        .pending
        .iter()
        .filter_map(|pending| pending.ownership.as_ref())
        .find(|conversation| {
            conversation.address.resident == address.resident
                && (directory.same(&conversation.address.machine, &address.machine)
                    || conversation.previous.iter().any(|machine| directory.same(machine, &address.machine)))
                && crate::owned_records::allows(directory, actor, conversation)
        })
        .cloned()
}

pub(super) fn targets(
    store: &Store,
    forgetting: &Forgetting,
    ownership: Option<&Conversation>,
) -> BTreeSet<MachineName> {
    let directory = &store.book.directory;
    let conversation = &forgetting.conversation;
    let mut targets: BTreeSet<_> = directory
        .members
        .iter()
        .filter(|member| {
            member.logins.iter().any(|account| match ownership {
                Some(ownership) => crate::owned_records::allows(directory, account, ownership),
                None => account == &forgetting.account,
            })
        })
        .map(|member| member.name.clone())
        .collect();
    for (machine, worked) in &store.worked {
        if worked.resident == conversation.resident
            && (directory.same(&worked.machine, &conversation.machine)
                || ownership.is_some_and(|ownership| {
                    ownership.previous.iter().any(|previous| directory.same(previous, &worked.machine))
                }))
            && let Some(member) = directory.member(machine)
        {
            targets.insert(member.name.clone());
        }
    }
    targets.retain(|member| !directory.same(member, &conversation.machine));
    targets
}
