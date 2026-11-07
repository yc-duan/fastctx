//! Local projections stay available during handoff when the target account opts in.

use crate::Node;
use fastctx_types::{access::HubAccount, history::Scope};

/// Importers maintain this small index; each tool avoids scanning every conversation's disk.
pub(crate) fn scopes(node: &Node) -> Vec<Scope> {
    node.history_imports.copies.lock().expect("never poisoned").values().cloned().collect()
}

pub(crate) fn enabled(node: &Node, scope: &Scope) -> Result<bool, String> {
    if !node.connected_anywhere() {
        return Ok(super::super::switch::alone(node));
    }
    if !fastctx_platform::history() || !super::pending(node, scope) {
        return Ok(false);
    }
    let account = match scope {
        Scope::Project { project } => {
            match crate::ledger::carry::target(&node.world, project).map_err(|error| error.to_string())? {
                Some(target) => Some(HubAccount { hub: target.place.hub, account: target.account }),
                None => registered_account(node),
            }
        }
        Scope::Conversation { resident, .. } => {
            if node.residents.active(resident).is_err() {
                return Ok(false);
            }
            let records = super::super::local::directory(node, scope);
            match super::state::read(&records).map_err(|error| error.to_string())? {
                Some(transfer) => Some(transfer.account),
                None => match node.residents.about(resident) {
                    Some((about, _)) => {
                        crate::placement::account(node, &about.acting, None).map_err(|error| error.message)?
                    }
                    None => None,
                },
            }
        }
    };
    Ok(account.is_some_and(|account| super::super::switch::enabled(node, &account)))
}

// Before the import chooses a hub, one registered account is unambiguous.
fn registered_account(node: &Node) -> Option<HubAccount> {
    let mut homes = node.links().into_iter().filter_map(|link| {
        let view = link.view();
        let home = view.me()?.home.clone()?;
        view.logged_in(&view.me, &home).then(|| HubAccount { hub: link.hub().clone(), account: home })
    });
    let first = homes.next();
    if homes.next().is_none() { first } else { None }
}
