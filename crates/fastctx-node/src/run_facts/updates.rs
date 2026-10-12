//! Executor updates use the existing durable letter transport.

use super::*;
use fastctx_types::run::{JobRecord, RunState};
use fastctx_types::run_facts::JobFacts;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

static EMITTING: Mutex<()> = Mutex::new(());

#[derive(Default, Serialize, Deserialize)]
struct Emitted {
    sequence: u64,
    state: Option<RunState>,
}

pub(crate) fn job(node: &Node, record_path: &Path, writes: Vec<WriteFact>) -> io::Result<()> {
    let _emitting = EMITTING.lock().expect("fact emitter lock");
    let record = fastctx_hands::run::record::read(record_path)?;
    let Some(call_id) = record.call_id.clone() else {
        return Ok(());
    };
    if record.owner.resident().is_none() {
        return Ok(());
    }
    let pending = record_path.with_extension("facts-pending");
    match std::fs::read(&pending) {
        Ok(bytes) => {
            let update: JobFacts = serde_json::from_slice(&bytes)?;
            deliver(node, &record, &update)?;
            std::fs::remove_file(&pending)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let state = crate::jobs::state(&record);
    let saved = record_path.with_extension("facts-state");
    let mut emitted: Emitted = match std::fs::read(&saved) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Emitted::default(),
        Err(error) => return Err(error),
    };
    if writes.is_empty() && emitted.state.as_ref() == Some(&state) {
        return Ok(());
    }
    emitted.sequence =
        emitted.sequence.checked_add(1).ok_or_else(|| io::Error::other("job facts sequence exhausted"))?;
    emitted.state = Some(state.clone());
    let update = JobFacts {
        call_id,
        primary: record.on.is_none(),
        // Job records have no measured output extent; the fitted call answer supplies it.
        output: None,
        visited: crate::jobs::visited(record_path.parent().expect("job record has a directory"), &record.id),
        machine: record.machine.clone(),
        job: record.id.clone(),
        sequence: emitted.sequence,
        state: Some(state),
        writes,
    };
    let pending = record_path.with_extension("facts-pending");
    publish(&pending, &serde_json::to_vec(&update)?, Permissions::Private)?;
    publish(&saved, &serde_json::to_vec(&emitted)?, Permissions::Private)?;
    deliver(node, &record, &update)?;
    std::fs::remove_file(pending)?;
    Ok(())
}

fn deliver(node: &Node, record: &JobRecord, update: &JobFacts) -> io::Result<()> {
    let owner = record.owner.resident().expect("a fact update has a resident owner");
    if owner.machine == node.machine() {
        return storage::receive(node, &owner.resident, update);
    }
    let acting = record
        .on
        .as_ref()
        .and_then(|on| on.account.clone())
        .or_else(|| record.account.clone())
        .map(|account| fastctx_types::access::Acting::one(account.hub, account.account))
        .unwrap_or_default();
    let id =
        crate::speech::fixed_id("runfacts", &[record.machine.key(), record.id.as_str(), &update.sequence.to_string()]);
    crate::letters::send(
        node,
        &acting,
        &owner.machine.clone().into(),
        id,
        fastctx_types::remote::Letter::JobFacts { to: owner.resident.clone(), update: Box::new(update.clone()) },
    )
}
