//! Startup owns the source until a pinned snapshot has durable open permission.
use super::{
    Event, FailureKind, Journal, MigrationGate, Options, Phase, Record, Transition,
    files::{self, Files},
    snapshot::{Budget, Failure, Snapshot, WorkResult},
};
use crate::{Database, Error, ErrorKind, Result};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

/// Run once under exclusive service-startup ownership, after schema commit and
/// before handing the database to services. Only pinning and journal commits are
/// synchronous; copying and verification run on one background thread. The journal
/// must be bound to this same database. An Err has NOT authorized service writes.
pub fn start(database: &Database, journal: Arc<dyn Journal>, options: Options) -> Result<MigrationGate> {
    database.available()?;
    if let Some(gate) = database.format_upgrade_gate() {
        return Ok(gate);
    }
    let (gate, leader) = super::registry::claim(database.path(), options.limits.elapsed)?;
    if leader {
        let result = start_owned(database, journal, options, gate.clone());
        gate.opened(result.as_ref().map(|_| ()).map_err(Clone::clone));
        if let Err(error) = result {
            gate.finish(Err(error.clone()));
            return Err(error);
        }
    } else {
        gate.wait_opened()?;
    }
    let _ = database.0.upgrade.set(gate.clone());
    Ok(gate)
}
fn start_owned(database: &Database, journal: Arc<dyn Journal>, options: Options, gate: MigrationGate) -> Result<()> {
    let files = Files::new(database.path())?;
    let mut record = journal.current()?;
    if record.phase == Phase::Copying {
        let name = if record.attempt == 0 { Some(files::name(&record, 1)?) } else { None };
        let (abandoned, retry) = record.interrupted(name.clone())?;
        let wanted = retry.unwrap_or(abandoned);
        record = reconcile(journal.as_ref(), &record, &wanted, || journal.interrupted(record.clone(), name.clone()))?;
    }
    for previous in journal.attempts()? {
        if matches!(previous.phase, Phase::Abandoned | Phase::Complete) {
            cleanup(&files, &previous, &options);
        }
    }
    if record.phase.finished() {
        report(&record, &options);
        gate.finish(Ok(record));
    } else {
        let work = prepare(database, &files, &record, &options, &gate);
        match work {
            Err(failure) => {
                record = fail(journal.as_ref(), &record, failure)?;
                report(&record, &options);
                cleanup(&files, &record, &options);
                gate.finish(Ok(record));
            }
            Ok((snapshot, budget)) => {
                if snapshot.is_some() {
                    let transition = Transition::Pinned {
                        at_millis: files::now()?,
                        file_name: record.file_name.clone().unwrap_or(files::name(&record, record.attempt)?),
                    };
                    (options.observe)(Event::SnapshotPinned(record.advance(&transition)?));
                    record = advance(journal.as_ref(), &record, transition)?;
                }
                // No user writer has seen the database yet; this receipt is the
                // durable boundary after which recovery must call the snapshot T1.
                (options.observe)(Event::ServicesReleased(record.clone()));
                let worker = Worker {
                    journal: journal.clone(),
                    files: files.clone(),
                    record: record.clone(),
                    options: options.clone(),
                    budget,
                    snapshot,
                };
                let completed = gate.clone();
                let recovery_journal = journal.clone();
                let warnings = options.observe.clone();
                if let Err(error) = std::thread::Builder::new().name("format-backup".into()).spawn(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| worker.run()));
                    let result = match result {
                        Ok(result) => result,
                        Err(_) => recovery_journal.current().and_then(|current| {
                            if current.phase.finished() {
                                return Ok(current);
                            }
                            let kind =
                                if current.phase == Phase::Copied { FailureKind::Integrity } else { FailureKind::Copy };
                            fail(
                                recovery_journal.as_ref(),
                                &current,
                                Failure::new(kind, Error::new("backup worker panicked; snapshot released")),
                            )
                        }),
                    };
                    // Even journal failure cannot pin a WAL or close migration
                    // forever. The warning is explicit; restart reconciles the record.
                    // Completion is published even when a diagnostic callback panics.
                    drop(recovery_journal);
                    completed.finish(result.clone());
                    if let Err(error) = result {
                        let message = format!(
                            "backup execution or durable accounting failed: {error}; no successful backup is claimed"
                        );
                        if catch_unwind(AssertUnwindSafe(|| warnings(Event::Warning(message.clone())))).is_err() {
                            eprintln!("format upgrade: {message}");
                        }
                    }
                }) {
                    record = fail(
                        journal.as_ref(),
                        &record,
                        Failure::new(
                            if record.phase == Phase::Copied { FailureKind::Publication } else { FailureKind::Copy },
                            error,
                        ),
                    )?;
                    report(&record, &options);
                    gate.finish(Ok(record));
                }
            }
        }
    }
    Ok(())
}

fn prepare(
    database: &Database,
    files: &Files,
    record: &Record,
    options: &Options,
    gate: &MigrationGate,
) -> WorkResult<(Option<Snapshot>, Budget)> {
    let budget =
        Budget::new(database.path(), options.limits.clone(), gate.deadline(), gate.stop_signal()).map_err(|e| {
            Failure::new(if record.phase == Phase::Copied { FailureKind::Integrity } else { FailureKind::Snapshot }, e)
        })?;
    if record.phase == Phase::Copied {
        return Ok((None, budget));
    }
    files.prepare().map_err(|e| Failure::new(FailureKind::Space, e))?;
    let snapshot = Snapshot::pin(database.path()).map_err(|e| Failure::new(FailureKind::Snapshot, e))?;
    budget.space(&files.directory, snapshot.logical_bytes)?;
    Ok((Some(snapshot), budget))
}
struct Worker {
    journal: Arc<dyn Journal>,
    files: Files,
    record: Record,
    options: Options,
    budget: Budget,
    snapshot: Option<Snapshot>,
}
impl Worker {
    fn run(mut self) -> Result<Record> {
        if let Err(error) = fastctx_platform::disks::lower_this_thread() {
            (self.options.observe)(Event::Warning(format!("could not lower backup thread priority: {error}")));
        }
        let result = self.work();
        // A failed file creation can precede taking the snapshot out of self.
        drop(self.snapshot.take());
        match result {
            Ok(()) => {}
            Err(WorkError::Physical(failure)) => {
                self.record = fail(self.journal.as_ref(), &self.record, failure)?;
            }
            Err(WorkError::PublicationPending(error)) => {
                (self.options.observe)(Event::Warning(format!(
                    "backup publication is unconfirmed: {error}; Copied evidence and names are retained for restart reconciliation"
                )));
                return Err(error);
            }
            Err(WorkError::Journal(error)) => {
                (self.options.observe)(Event::Warning(format!(
                    "backup status could not be committed: {error}; restart will reconcile it"
                )));
                return Err(error);
            }
        }
        if matches!(self.record.phase, Phase::Abandoned | Phase::Complete) {
            cleanup(&self.files, &self.record, &self.options);
        }
        report(&self.record, &self.options);
        Ok(self.record)
    }
    fn work(&mut self) -> std::result::Result<(), WorkError> {
        if let Some(snapshot) = self.snapshot.take() {
            let path = self.files.create(&self.record).map_err(|e| Failure::new(FailureKind::Copy, e))?;
            let pages = snapshot.copy(&path, &self.budget, &self.options.observe)?;
            (self.options.observe)(Event::CopyFlushed { pages });
            self.record = advance(self.journal.as_ref(), &self.record, Transition::Copied { pages })
                .map_err(WorkError::Journal)?;
            (self.options.observe)(Event::Copied(self.record.clone()));
        }
        let path = self.files.copied_path(&self.record).map_err(|e| Failure::new(FailureKind::Integrity, e))?;
        super::verify::verify(
            &path,
            self.record
                .copied_pages
                .ok_or_else(|| WorkError::Journal(Error::new("copied receipt has no page count")))?,
            &self.budget,
        )?;
        (self.options.observe)(Event::IntegrityChecked(self.record.clone()));
        self.budget.time()?;
        self.budget.require_running(FailureKind::Publication)?;
        self.files.publish(&self.record).map_err(WorkError::PublicationPending)?;
        (self.options.observe)(Event::Published(self.record.clone()));
        self.record = advance(self.journal.as_ref(), &self.record, Transition::Complete).map_err(WorkError::Journal)?;
        Ok(())
    }
}
enum WorkError {
    Physical(Failure),
    Journal(Error),
    PublicationPending(Error),
}
impl From<Failure> for WorkError {
    fn from(failure: Failure) -> Self {
        Self::Physical(failure)
    }
}
fn advance(journal: &dyn Journal, record: &Record, transition: Transition) -> Result<Record> {
    let wanted = record.advance(&transition)?;
    reconcile(journal, record, &wanted, || journal.advance(record.clone(), transition.clone()))
}
fn reconcile(
    journal: &dyn Journal,
    before: &Record,
    wanted: &Record,
    submit: impl Fn() -> Result<Record>,
) -> Result<Record> {
    match submit() {
        Ok(record) => Ok(record),
        Err(error) if matches!(error.kind(), ErrorKind::OutcomeUnknown | ErrorKind::Busy) => {
            let actual = journal.current()?;
            if &actual == wanted {
                return Ok(actual);
            }
            if &actual != before {
                return Err(Error::new("format-upgrade owner changed during reconciliation"));
            }
            // Same expected value and transition: queued original and retry cannot
            // create two transitions. Never invent another attempt after timeout.
            submit()
        }
        Err(error) => Err(error),
    }
}
fn fail(journal: &dyn Journal, record: &Record, failure: Failure) -> Result<Record> {
    let mut detail = failure.error.to_string();
    if detail.len() > 4096 {
        let mut end = 4096;
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
    }
    advance(journal, record, Transition::Failed { kind: failure.kind, detail })
}
fn cleanup(files: &Files, record: &Record, options: &Options) {
    if let Err(error) = files.clean_partial(record) {
        (options.observe)(Event::Warning(format!(
            "could not remove upgrade partial for T{}: {error}; retained for next startup cleanup",
            record.attempt
        )));
    }
}
fn report(record: &Record, options: &Options) {
    if record.attempt == 1 {
        (options.observe)(Event::Warning(format!(
            "T0 was interrupted; T1 is a post-upgrade recovery snapshot at {:?} milliseconds since epoch, not the original upgrade snapshot",
            record.snapshot_at_millis
        )));
    }
    if record.phase == Phase::Abandoned {
        (options.observe)(Event::Warning(format!(
            "{} format {} backup T{} abandoned: {}; service and migration may continue",
            record.service.name(),
            record.format,
            record.attempt,
            record.reason.as_deref().unwrap_or("unknown reason")
        )));
    }
    (options.observe)(Event::Finished(record.clone()));
}
