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
    let path = directory.books().join("goal-account.json");
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| Failure::new(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::new(error.to_string())),
    }
}

pub(crate) fn resolve(
    node: &Node,
    resident: &ResidentId,
    selected: Option<&HubAccount>,
) -> Result<HubAccount, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
    let pinned = pinned(node, resident)?;
    if let (Some(pinned), Some(selected)) = (&pinned, selected)
        && pinned != selected
    {
        return Err(Failure::not_run(words::ACCOUNT_CHANGED));
    }
    let choices: BTreeSet<_> = node
        .links()
        .iter()
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
    if pinned_at(directory)
        .map_err(|error| std::io::Error::other(error.message))?
        .is_some_and(|pinned| &pinned != account)
    {
        return Err(std::io::Error::other(words::ACCOUNT_CHANGED));
    }
    let path = directory.books().join("goal-account.json");
    fastctx_platform::ensure_private_dir(path.parent().expect("goal binding directory"))?;
    fastctx_platform::publish(&path, &serde_json::to_vec(account)?, fastctx_platform::Permissions::Private)
}
