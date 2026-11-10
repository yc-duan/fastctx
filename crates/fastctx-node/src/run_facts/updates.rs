//! Executor updates use the existing durable letter transport.

use super::*;
use fastctx_types::run::{JobRecord, RunState};
use fastctx_types::run_facts::JobFacts;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static EMITTING: LazyLock<crate::record_gates::Gates> =
    LazyLock::new(|| crate::record_gates::Gates::new(fastctx_platform::doing::Rank::EMITTING, "call facts"));

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Emitted {
    pub(super) sequence: u64,
    #[serde(default)]
    journal_version: u8,
    state: Option<RunState>,
    #[serde(default)]
    output_ended: bool,
    #[serde(default)]
    writes_offset: u64,
    #[serde(default)]
    steps_offset: u64,
    #[serde(default)]
    steps_segment: u64,
    #[serde(default)]
    visits_offset: u64,
    #[serde(default)]
    sql_members: bool,
    #[serde(default)]
    metadata_sent: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Pending {
    update: JobFacts,
    emitted: Emitted,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum Recovery {
    Current(Box<Pending>),
    Legacy(Box<JobFacts>),
}

/// Emits only journal rows not already placed in the durable transport.
/// The pending letter and its cursor are published before delivery, so a crash
/// can replay the same sequence without skipping any committed row.
pub(crate) fn job(node: &Node, record_path: &Path) -> io::Result<()> {
    let before = fastctx_hands::run::record::read(record_path)?;
    super::step_view::report(node, super::step_view::refresh(record_path));
    if let Err(error) = crate::activity_book::observe(node, record_path) {
        // The journal remains pinned by the book cursor; its separate database
        // must not prevent the job's owner from hearing progress or completion.
        node.log.line(format_args!("recording activity for job {}: {error}", before.id));
    }
    if before.call_id.is_none() || before.owner.resident().is_none() {
        return Ok(());
    }
    let gate = EMITTING.get(record_path);
    let _emitting = gate.lock();
    let record = fastctx_hands::run::record::read(record_path)?;
    let call_id = record.call_id.clone().expect("a call was recorded");
    let metadata = crate::job_metadata::Job::of(record_path, &record)?;
    super::frame_queue::drain(&metadata, |update| deliver(node, &record, update))?;
    if let Some(bytes) = metadata.raw("facts-pending")? {
        let (update, mut emitted) = match serde_json::from_slice::<Recovery>(&bytes)? {
            Recovery::Current(pending) => (pending.update, pending.emitted),
            Recovery::Legacy(update) => {
                let mut emitted = metadata.get::<Emitted>("facts-state")?.unwrap_or_default();
                emitted.sequence = emitted.sequence.max(update.sequence);
                emitted.state = update.state.clone();
                emitted.output_ended = update.output.is_some();
                (*update, emitted)
            }
        };
        super::frame_queue::publish(&metadata, update, &mut emitted)?;

        super::frame_queue::drain(&metadata, |update| deliver(node, &record, update))?;
    }
    let mut emitted = metadata.get::<Emitted>("facts-state")?.unwrap_or_default();
    super::writing::recover(record_path, emitted.journal_version == 0)?;
    if !emitted.sql_members {
        emitted.writes_offset = 0;
        emitted.visits_offset = 0;
        emitted.sql_members = true;
        emitted.metadata_sent = emitted.sequence > 0;
        emitted.journal_version = 1;
        metadata.put("facts-state", &emitted)?;
    }
    loop {
        let writes = super::read_since(record_path, &mut emitted.writes_offset)?;
        let visited = super::visits::page(record_path, &mut emitted.visits_offset)?;
        let steps = {
            let gate = WRITING.get(&record_path.with_extension("facts"));
            let _writing = gate.lock();
            super::journal::read_since(record_path, &mut emitted.steps_segment, &mut emitted.steps_offset)?
        };
        let state = crate::jobs::state(&record);
        let output_ended = record.output_ended_at.is_some();
        let changed = emitted.state.as_ref() != Some(&state) || emitted.output_ended != output_ended;
        if writes.is_empty() && steps.is_empty() && visited.is_empty() && !changed {
            return Ok(());
        }
        let owner = record.owner.resident().expect("a resident owns the facts");
        let incremental = node.is_me(&owner.machine)
            || crate::facts::cached(node, &owner.machine.clone().into()).is_some_and(|facts| facts.job_fact_deltas);
        let initial = !emitted.metadata_sent || !incremental;
        let (output, notes) = if output_ended && (!emitted.output_ended || !incremental || !writes.is_empty()) {
            let (output, notes) = super::output::ended(node, record_path, &record)?;
            (Some(output), Some(notes))
        } else {
            (None, None)
        };
        emitted.metadata_sent = true;
        emitted.state = Some(state.clone());
        emitted.output_ended = output_ended;
        let update = JobFacts {
            record: initial.then(|| record_path.to_string_lossy().into_owned()),
            script: initial.then(|| record.script.clone()),
            started_at: initial.then_some(record.started_at),
            finished_at: record.end.as_ref().map(|end| end.at),
            turn: record.turn.clone(),
            call_id: call_id.clone(),
            primary: record.on.is_none(),
            output,
            notes,
            visited,
            machine: record.machine.clone(),
            job: record.id.clone(),
            sequence: emitted.sequence,
            state: Some(state),
            writes,
            steps,
        };
        super::frame_queue::publish(&metadata, update, &mut emitted)?;
        super::frame_queue::drain(&metadata, |update| deliver(node, &record, update))?;
    }
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
    let id = crate::speech::fixed_id(
        "runfacts",
        &[
            record.machine.key(),
            owner.machine.key(),
            owner.resident.as_str(),
            record.id.as_str(),
            &record.started_at.to_string(),
            &update.sequence.to_string(),
        ],
    );
    crate::letters::send(
        node,
        &acting,
        &owner.machine.clone().into(),
        id,
        fastctx_types::remote::Letter::JobFacts { to: owner.resident.clone(), update: Box::new(update.clone()) },
    )
}

pub(super) fn segment(record: &Path) -> io::Result<u64> {
    Ok(crate::job_metadata::Job::open(record)?.get::<Emitted>("facts-state")?.unwrap_or_default().steps_segment)
}
