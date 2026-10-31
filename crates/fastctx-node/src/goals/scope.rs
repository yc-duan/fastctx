//! A goal's account is selected once; changes in directory order never move it to another account.

use super::words;
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::access::HubAccount;
use fastctx_types::ipc::Failure;
use std::collections::BTreeSet;

pub(crate) fn pinned(node: &Node, resident: &ResidentId) -> Result<Option<HubAccount>, Failure> {
    pinned_at(&node.world.conversation(resident))
}

pub(crate) fn pinned_at(directory: &fastctx_types::layout::ConversationDir) -> Result<Option<HubAccount>, Failure> {
    initialize(directory).map_err(|error| Failure::new(error.to_string()))?;
    crate::records::read(directory, |rows| rows.get("goal-account", "current"))
        .map_err(|error| Failure::new(error.to_string()))
}
fn initialize(directory: &fastctx_types::layout::ConversationDir) -> std::io::Result<()> {
    crate::records::legacy::single::<HubAccount>(
        directory,
        "goal-account",
        &directory.books().join("goal-account.json"),
    )
}

pub(crate) fn resolve(
    node: &Node,
    resident: &ResidentId,
    selected: Option<&HubAccount>,
) -> Result<HubAccount, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    let requested_hub = selected.map(|account| account.hub.clone());
    let selected = match selected {
        Some(account) if !node.serves(&about.acting, &account.hub, &account.account) => {
            if !crate::record_ownership::allows(node, &about, &account.hub, &account.account) {
                return Err(Failure::refused("this account does not own the conversation's goal"));
            }
            None
        }
        selected => selected,
    };
    let pinned = pinned(node, resident)?;
    if pinned.as_ref().is_some_and(|pinned| requested_hub.as_ref().is_some_and(|hub| hub != &pinned.hub)) {
        return Err(Failure::not_run(words::ACCOUNT_CHANGED));
    }
    if let (Some(pinned), Some(selected)) = (&pinned, selected)
        && pinned != selected
    {
        return Err(Failure::not_run(words::ACCOUNT_CHANGED));
    }
    let choices: BTreeSet<_> = node
        .links()
        .iter()
        .filter(|link| requested_hub.as_ref().is_none_or(|hub| link.membership().id == *hub))
        .flat_map(|link| {
            node.accounts_on(link, &about.acting)
                .into_iter()
                .map(|account| HubAccount { hub: link.membership().id.clone(), account })
                .collect::<Vec<_>>()
        })
        .collect();
    if let Some(account) = pinned.as_ref().or(selected) {
        return if node.serves(&about.acting, &account.hub, &account.account) {
            Ok(account.clone())
        } else {
            Err(Failure::not_run(words::ACCOUNT_MISSING))
        };
    }
    match choices.len() {
        1 => Ok(choices.into_iter().next().expect("one account")),
        0 => Err(Failure::not_run(words::ACCOUNT_MISSING)),
        _ => Err(Failure::not_run(words::ACCOUNT_AMBIGUOUS)),
    }
}

pub(crate) fn keep(node: &Node, resident: &ResidentId, account: &HubAccount) -> std::io::Result<()> {
    keep_at(&node.world.conversation(resident), account)
}

pub(crate) fn keep_at(directory: &fastctx_types::layout::ConversationDir, account: &HubAccount) -> std::io::Result<()> {
    initialize(directory)?;
    let account = account.clone();
    crate::records::write_at(directory, fastctx_storage::Priority::Interactive, 256, move |rows| {
        if rows.get::<HubAccount>("goal-account", "current")?.is_some_and(|prior| prior != account) {
            return Err(fastctx_storage::Error::new(words::ACCOUNT_CHANGED));
        }
        rows.put("goal-account", "current", &account)
    })
}

pub(crate) fn user(node: &Node, resident: &ResidentId, selected: Option<&HubAccount>) -> Result<HubAccount, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    let mut choices = Vec::new();
    for link in node.links() {
        for name in node.accounts_on(&link, &fastctx_types::access::Acting::Here) {
            let account = HubAccount { hub: link.membership().id, account: name };
            if selected.is_none_or(|selected| selected == &account)
                && crate::record_ownership::allows(node, &about, &account.hub, &account.account)
            {
                choices.push(account);
            }
        }
    }
    if choices.len() == 1 {
        return Ok(choices.remove(0));
    }
    let owner = resolve(node, resident, selected)?;
    if choices.contains(&owner) {
        return Ok(owner);
    }
    Err(Failure::refused(words::ACCOUNT_AMBIGUOUS))
}
