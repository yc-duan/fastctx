//! A shell write belongs to the resident running that shell, wherever it lives.

use crate::Node;
use crate::town::caller::{self, Caller};
use fastctx_types::TownName;
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::write::{WriteOp, WriteOutcome};

pub(crate) async fn write(
    node: &Node,
    peer: &Peer,
    town: Option<TownName>,
    op: WriteOp,
) -> Result<WriteOutcome, Failure> {
    let Caller::Resident(owner) = caller::of(node, peer)? else {
        return Err(Failure::new("write commands need a resident identity; invoke them through the World's run tool"));
    };
    let here = node.machine();
    let town = if owner.machine == here {
        node.residents
            .town_of(&owner.resident)
            .ok_or_else(|| Failure::new(format!("no resident {} lives on {here}", owner.resident)))?
    } else {
        town
    };
    let world = node.world.clone();
    tokio::task::spawn_blocking(move || {
        let at = jiff::Timestamp::now();
        let outcome = crate::write::write_here(&world, &here, &owner, town.as_ref(), op.clone())?;
        let action = crate::write::action(&op, &outcome, &here, at);
        crate::residents::record_owned_action(&world, &here, &owner, town.as_ref(), &action)?;
        std::io::Result::Ok(outcome)
    })
    .await
    .map_err(|error| Failure::new(format!("writing: {error}")))?
    .map_err(|error| Failure::new(format!("writing: {error}")))
}
