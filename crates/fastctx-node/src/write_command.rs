//! A shell write belongs to the resident running that shell, wherever it lives.

use crate::Node;
use crate::town::caller::{self, Caller};
use fastctx_types::TownName;
use fastctx_types::ipc::{Failure, Peer};
use fastctx_types::write::{WriteOp, WriteOutcome};

pub(crate) async fn write(
    node: &std::sync::Arc<Node>,
    peer: &Peer,
    town: Option<TownName>,
    op: WriteOp,
    job: Option<fastctx_types::JobId>,
) -> Result<WriteOutcome, Failure> {
    let caller = caller::of(node, peer)?;
    node.authorize_here(&crate::around::acting_of(node, &caller))?;
    let Caller::Resident(owner) = &caller else {
        return Err(Failure::new("edit and write work only inside a script run by the run tool"));
    };
    let here = node.names();
    let town = if node.is_me(&owner.machine) {
        node.residents
            .town_of(&owner.resident)
            .ok_or_else(|| Failure::new(format!("no resident {} lives on {}", owner.resident, here.me)))?
    } else {
        town
    };
    let memory = match crate::replicas::writing::intercept(node, &caller, &op, job.as_ref(), town.as_ref()).await? {
        Some(outcome) => Some(outcome),
        None => crate::history::local_copy::intercept(node, &caller, &op, job.as_ref(), town.as_ref()).await?,
    };
    let owner = owner.clone();
    let world = node.world.clone();
    let record_path = job.as_ref().map(|job| node.world.job_record(&here, town.as_ref(), &owner.clone().into(), job));
    let action_record = record_path.clone();
    let result = fastctx_platform::task::spawn_blocking(move || {
        let at = jiff::Timestamp::now();
        let turn =
            action_record.as_deref().map(fastctx_hands::run::record::read).transpose()?.and_then(|record| record.turn);
        let outcome = match memory {
            Some(outcome) => outcome,
            None => crate::write::write_job(&world, &here, &owner, town.as_ref(), op.clone(), job.as_ref())?,
        };
        let mut action = crate::write::action(&op, &outcome, &here.me, at);
        action.turn = turn;
        let recorded = crate::residents::record_owned_action(&world, &here, &owner, &action);
        std::io::Result::Ok((outcome, recorded))
    })
    .await
    .map_err(|error| Failure::new(format!("writing: {error}")))?
    .map_err(|error| Failure::new(format!("writing: {error}")))?;
    let (outcome, recorded) = result;
    if let Some(path) = record_path {
        let emitting = node.clone();
        let record = fastctx_platform::task::spawn_blocking(move || {
            crate::run_facts::update_job(&emitting, &path)?;
            fastctx_hands::run::record::read(&path)
        })
        .await
        .map_err(|error| Failure::new(format!("updating call facts: {error}")))?
        .map_err(|error| Failure::new(format!("updating call facts: {error}")))?;
        node.jobs.notify(record.id);
    }
    recorded.map_err(|error| Failure::new(format!("writing: {error}")))?;
    Ok(outcome)
}
