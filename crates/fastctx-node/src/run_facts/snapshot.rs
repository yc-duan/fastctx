//! Applying a fact frame is independent of the node, its files and its network.

use super::{projections, sql};
use crate::record_payloads::{self as payloads, Reference};
use fastctx_storage::{Result as StoredResult, rusqlite::Connection};
use fastctx_types::run_facts::{JobFacts, RunFacts};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct StoredCall {
    pub call_id: String,
    pub facts: RunFacts,
    pub sequence: u64,
    pub launched: bool,
    #[serde(default)]
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

pub(super) fn terminal(state: &fastctx_types::run::RunState) -> bool {
    !matches!(state, fastctx_types::run::RunState::Running | fastctx_types::run::RunState::Exited { .. })
}

pub(super) fn validate(stored: &StoredCall, update: &JobFacts) -> StoredResult<()> {
    if stored.call_id != update.call_id {
        return Err(fastctx_storage::Error::new("call fact identity changed"));
    }
    if update.turn.is_some() && stored.facts.turn.is_some() && update.turn != stored.facts.turn {
        return Err(fastctx_storage::Error::new("job facts belong to a different turn"));
    }
    Ok(())
}

pub(super) fn apply(stored: &mut StoredCall, update: &JobFacts) -> StoredResult<()> {
    validate(stored, update)?;
    stored.launched = true;
    stored.facts.turn = stored.facts.turn.take().or(update.turn.clone());
    if update.primary && stored.facts.job.is_none() {
        stored.facts.machine = update.machine.clone();
        stored.facts.job = Some(update.job.clone());
        stored.facts.output = update.output.clone();
    }
    if update.primary && stored.facts.machine == update.machine && stored.facts.job.as_ref() == Some(&update.job) {
        stored.facts.record = stored.facts.record.take().or(update.record.clone());
    }
    if update.primary && stored.facts.machine == update.machine && stored.facts.job.as_ref() == Some(&update.job) {
        let current = update.sequence >= stored.sequence;
        if let Some(output) = &update.output {
            if let Some(previous) = &mut stored.facts.output {
                if previous.machine == output.machine
                    && previous.path == output.path
                    && (current || (previous.content.is_none() && output.content.is_some()))
                {
                    previous.cut |= output.bytes > previous.bytes;
                    previous.content = output.content.clone();
                    previous.policy = output.policy.clone();
                    previous.bytes = output.bytes;
                    previous.lines = output.lines;
                }
            } else {
                stored.facts.output = Some(output.clone());
            }
        }
        if current {
            if let Some(state) = &update.state
                && (!terminal(&stored.facts.state) || terminal(state))
            {
                stored.facts.state = state.clone();
            }
            stored.sequence = update.sequence;
        }
    }
    Ok(())
}

pub(super) fn receive_in(
    db: &Connection,
    conversation: &str,
    update: &JobFacts,
    notes: Option<&Reference>,
) -> StoredResult<bool> {
    let Some(mut stored) = sql::head(db, conversation, &update.call_id)? else { return Ok(false) };
    // Sequence orders a job's state, not the membership of its facts: an
    // earlier frame can still carry steps absent from a later one.
    let before = db.total_changes();
    let current = update.sequence >= stored.sequence;
    apply(&mut stored, update)?;
    if let Some(notes) = notes
        && update.primary
        && stored.facts.machine == update.machine
        && stored.facts.job.as_ref() == Some(&update.job)
        && (current
            || payloads::root(db, conversation, projections::NOTES, &update.call_id)?
                .is_none_or(|root| root.bytes == 2))
    {
        payloads::link(db, conversation, projections::NOTES, &update.call_id, Some(notes))?;
    }
    sql::writes(db, conversation, &update.call_id, &update.writes)?;
    sql::steps(db, conversation, &update.call_id, &update.steps)?;
    sql::visits(db, conversation, &update.call_id, &update.visited)?;
    let mut observed = update.clone();
    observed.turn = stored.facts.turn.clone();
    sql::save_job(db, conversation, &observed, notes)?;
    sql::save(db, conversation, &stored)?;
    let changed = db.total_changes() != before;
    if changed {
        sql::changed(db, conversation)?;
    }
    Ok(changed)
}
