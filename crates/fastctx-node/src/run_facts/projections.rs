//! Selected output and exact note matches are immutable payloads, never part of
//! the mutable progress head. Their roots commit with the owning facts.
use super::{snapshot::StoredCall, sql};
use crate::record_payloads::{self as payloads, Staged};
use fastctx_storage::{Database, Priority, Result, rusqlite::Connection};

pub(super) const NOTES: &str = "run-notes";
pub(super) const SPANS: &str = "run-spans";
pub(super) const JOB_NOTES: &str = "run-job-notes";

pub(super) struct Call {
    notes: Staged,
    spans: Staged,
}
impl Call {
    pub(super) fn prepare(
        database: &Database,
        scope: &str,
        stored: &mut StoredCall,
        priority: Priority,
    ) -> Result<Self> {
        let notes = payloads::stage(database, scope, sql::encode(&std::mem::take(&mut stored.facts.notes))?, priority)?;
        let spans = payloads::stage(database, scope, sql::encode(&std::mem::take(&mut stored.facts.spans))?, priority)?;
        Ok(Self { notes, spans })
    }
    pub(super) fn install(&self, db: &Connection, scope: &str, call: &str, preserve_notes: bool) -> Result<()> {
        if !preserve_notes {
            payloads::link(db, scope, NOTES, call, Some(&self.notes.reference))?;
        }
        payloads::link(db, scope, SPANS, call, Some(&self.spans.reference))?;
        self.finish(db, scope)
    }
    pub(super) fn finish(&self, db: &Connection, scope: &str) -> Result<()> {
        payloads::finish(db, scope, &self.notes)?;
        payloads::finish(db, scope, &self.spans)
    }
}
pub(super) fn job_key(update: &fastctx_types::run_facts::JobFacts) -> Result<String> {
    serde_json::to_string(&(&update.call_id, &update.machine, &update.job))
        .map_err(|error| fastctx_storage::Error::new(error.to_string()))
}
