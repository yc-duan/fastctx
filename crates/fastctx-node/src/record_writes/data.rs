//! Local data maintenance runs beyond the lifetime of the requesting connection.

use super::Asked;
use crate::{Node, submitted::Family};
use fastctx_types::{
    Call, Reply,
    data::DataCall,
    ipc::{Failure, Peer},
};
use std::sync::Arc;
use std::time::Duration;

const DELETE: Family = Family {
    name: "call/data-delete",
    // Each deletion still uses the existing machine-wide deletion slots.
    at_once: 4,
    // A batch can remove a large conversation tree and its host's records.
    within: Duration::from_secs(30 * 60),
    // Local data calls have no finite caller deadline; wait at most four minutes.
    stays: Duration::from_secs(240),
    // A repeated deletion cannot reproduce the original freed-byte result.
    again: false,
};

const CACHE: Family = Family {
    name: "call/data-cache",
    at_once: 1,
    // Cache clearing walks the whole cache, coordinating with active viewers.
    within: Duration::from_secs(30 * 60),
    stays: Duration::from_secs(240),
    // A second pass could delete cache entries produced after the first one.
    again: false,
};

fn check(node: &Node, asked: &Asked) -> Result<(), Failure> {
    crate::server::users_voice(node, &asked.peer)?;
    crate::admission::local::admit(node, &asked.peer, &asked.call)
}

pub(super) fn register(node: &Arc<Node>) {
    for family in [DELETE, CACHE] {
        crate::submitted::register(node, family, |node: Arc<Node>, asked: Asked| async move {
            check(&node, &asked)?;
            let Call::Data(call) = *asked.call else {
                return Err(Failure::not_run("the recorded maintenance call is not a data call"));
            };
            crate::data::answer(&node, call).await.map(Reply::Data)
        });
    }
}

pub(super) async fn local(node: &Arc<Node>, peer: &Peer, call: Call) -> Result<Reply, Failure> {
    let asked = Asked { peer: peer.clone(), call: Box::new(call) };
    check(node, &asked)?;
    let (family, unit) = match &*asked.call {
        Call::Data(DataCall::Delete { names }) => {
            // Keep the batch's original order and per-name failures. Its unit names
            // the target set; overlapping batches still use each place's own gate.
            let mut targets = names.clone();
            targets.sort();
            targets.dedup();
            (&DELETE, format!("data-places/{}", super::unit(&targets)?))
        }
        Call::Data(DataCall::ClearCache) => (&CACHE, "data-cache".to_string()),
        _ => return Err(Failure::not_run("this data call is not submitted work")),
    };
    let reply = crate::submitted::submit(node, family, &unit, &asked).await?;
    check(node, &asked)?;
    Ok(reply)
}
