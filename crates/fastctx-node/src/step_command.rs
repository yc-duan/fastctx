//! A step one of the World's commands tells from inside a run belongs to
//! the job whose script gave it, wherever its resident lives
//! (`fastctx_types::run_facts::RunStep`).

use crate::Node;
use crate::town::caller::{self, Caller};
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::run_facts::{RunStep, StepDid};
use fastctx_types::{JobId, TownName};

pub(crate) struct Told {
    pub job: JobId,
    pub town: Option<TownName>,
    pub id: String,
    pub began: jiff::Timestamp,
    pub ended: Option<jiff::Timestamp>,
    pub did: StepDid,
}

pub(crate) async fn stepped(node: &std::sync::Arc<Node>, peer: &Peer, told: Told) -> Result<(), Failure> {
    // An `on` is told by the node that carries it.
    if matches!(told.did, StepDid::On { .. }) {
        return Err(Failure::new("an on's steps are the node's to tell"));
    }
    let caller = caller::of(node, peer)?;
    node.authorize_here(&crate::around::acting_of(node, &caller))?;
    let Caller::Resident(owner) = &caller else {
        return Err(Failure::new("steps are told only inside a script run by the run tool"));
    };
    let here = node.names();
    let town = if node.is_me(&owner.machine) {
        node.residents
            .town_of(&owner.resident)
            .ok_or_else(|| Failure::new(format!("no resident {} lives on {}", owner.resident, here.me)))?
    } else {
        told.town
    };
    let path = node.world.job_record(&here, town.as_ref(), &owner.clone().into(), &told.job);
    let step = RunStep {
        id: told.id,
        job: told.job,
        machine: node.machine(),
        began: told.began,
        ended: told.ended,
        did: told.did,
    };
    let (node, owner) = (node.clone(), owner.clone());
    tokio::task::spawn_blocking(move || {
        let record = fastctx_hands::run::record::read(&path)?;
        if record.owner.resident() != Some(&owner) || record.end.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the step does not belong to an active job",
            ));
        }
        // A job no call waits on has no facts to join.
        if record.call_id.is_none() {
            return Ok(());
        }
        crate::run_facts::tell_step(&node, &path, step)
    })
    .await
    .map_err(|error| Failure::new(format!("keeping the step: {error}")))?
    .map_err(|error| Failure::new(format!("keeping the step: {error}")))
}
