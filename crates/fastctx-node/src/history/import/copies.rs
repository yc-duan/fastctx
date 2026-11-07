//! Local projections stay available during handoff when the target account opts in.

use crate::Node;
use fastctx_types::{access::HubAccount, history::Scope};

pub(crate) fn enabled(node: &Node, scope: &Scope) -> Result<bool, String> {
    if !node.connected_anywhere() {
        return Ok(super::super::switch::alone(node));
    }
    if !fastctx_platform::history() || !super::pending(node, scope) {
        return Ok(false);
    }
    let account = match scope {
        Scope::Project { project } => crate::ledger::carry::target(&node.world, project)
            .map_err(|error| error.to_string())?
            .map(|target| HubAccount { hub: target.place.hub, account: target.account }),
        Scope::Conversation { resident, .. } => {
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
