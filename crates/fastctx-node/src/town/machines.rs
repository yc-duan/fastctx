//! Machine names in project records belong to the project's hub, including
//! names retained in records written before a machine was renamed.

use fastctx_types::access::Written;
use fastctx_types::ipc::Failure;
use fastctx_types::{MachineName, TownName};

use crate::Node;

pub(crate) fn destination(node: &Node, town: &TownName, name: &MachineName) -> Result<Written, Failure> {
    let place = crate::ledger::names::place(&node.world, town)
        .map_err(|error| Failure::new(error.to_string()))?
        .ok_or_else(|| super::map::no_ledger(node, town))?;
    if let Some(link) = node.link_of(&place.hub) {
        return Ok(Written { name: name.clone(), hub: Some(link.hub_name()) });
    }
    if place.hub == crate::ledger::alone(&node.world).map_err(|error| Failure::new(error.to_string()))? {
        return Ok(name.into());
    }
    Err(Failure::new(format!("this computer is not connected to the hub of {town}")))
}
