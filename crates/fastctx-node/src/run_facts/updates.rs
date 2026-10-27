//! Executor updates use the existing durable letter transport.

use super::*;
use fastctx_types::run::{JobRecord, RunState};
use fastctx_types::run_facts::JobFacts;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static EMITTING: LazyLock<crate::ranked::Emitting> = LazyLock::new(Default::default);

#[derive(Default, Serialize, Deserialize)]
struct Emitted {
    sequence: u64,
    state: Option<RunState>,
    #[serde(default)]
    output_ended: bool,
}

pub(crate) fn job(node: &Node, record_path: &Path, writes: Vec<WriteFact>) -> io::Result<()> {
    job_with(node, record_path, writes, Vec::new())
}

/// Tells the job's owner what changed of it, with the writes and steps
/// just told.
pub(crate) fn job_with(
    node: &Node,
    record_path: &Path,
    writes: Vec<WriteFact>,
    steps: Vec<fastctx_types::run_facts::RunStep>,
) -> io::Result<()> {
    let _emitting = EMITTING.lock();
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
    let output_ended = record.output_ended_at.is_some();
    // A stopped capture can finish draining without changing its RunState.
    if writes.is_empty()
        && steps.is_empty()
        && emitted.state.as_ref() == Some(&state)
        && emitted.output_ended == output_ended
    {
        return Ok(());
    }
    emitted.sequence =
        emitted.sequence.checked_add(1).ok_or_else(|| io::Error::other("job facts sequence exhausted"))?;
    emitted.state = Some(state.clone());
    emitted.output_ended = output_ended;
    let (output, notes) = if record.output_ended_at.is_some() {
        let content = fastctx_hands::files::content_fingerprint(Path::new(&record.output))?;
        let index = fastctx_hands::lines::Index::open(
            Path::new(&record.output),
            node.world.path(),
            true,
            None,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .map_err(io::Error::other)?;
        let notes = fastctx_hands::run::projection::CapturedNotes::read(
            Path::new(&record.output),
            content.size,
            &super::writes(record_path)?,
        )?
        .matches();
        (
            Some(OutputRef {
                policy: Some(fastctx_hands::lines::output_policy()),
                machine: record.machine.clone(),
                path: record.output.clone(),
                bytes: content.size,
                lines: index.length,
                cut: false,
                content: Some(content),
            }),
            Some(notes),
        )
    } else {
        (None, None)
    };
    let update = JobFacts {
        record: Some(record_path.to_string_lossy().into_owned()),
        script: Some(record.script.clone()),
        started_at: Some(record.started_at),
        finished_at: record.end.as_ref().map(|end| end.at),
        turn: record.turn.clone(),
        call_id,
        primary: record.on.is_none(),
        output,
        notes,
        visited: crate::jobs::visited(record_path.parent().expect("job record has a directory"), &record.id),
        machine: record.machine.clone(),
        job: record.id.clone(),
        sequence: emitted.sequence,
        state: Some(state),
        writes,
        steps,
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
    if node.is_me(&owner.machine) {
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
