//! Local format-upgrade accounting, independent of account scopes and replication.
//!
//! The owning database identifies the data root; its identity moves with a copied
//! or relocated database. Only startup and upgrade composition receive a journal.

mod basis;
mod executor;
mod files;
mod gate;
mod names;
mod options;
mod snapshot;
mod state;
pub use executor::start;
pub use gate::MigrationGate;
pub use options::{Event, Limits, Observer, Options};

impl crate::Database {
    /// Present only for service databases whose startup installed an upgrade gate.
    pub fn format_upgrade_gate(&self) -> Option<MigrationGate> {
        self.0.upgrade.get().cloned()
    }
}
use crate::{Error, Result};
pub use basis::BackupBasis;

/// Version text is emitted in refusal messages, including records of newer formats.
pub fn validate_written_by(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(Error::new("invalid format writer version"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    World,
    Hub,
}
impl Service {
    pub const fn name(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Hub => "hub",
        }
    }
    pub const fn format(self) -> u32 {
        match self {
            Self::World => 4,
            Self::Hub => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Services have not been released by this attempt. T1 reserves its basename.
    Prepared,
    /// The snapshot was pinned and this fact committed before services may write.
    Copying,
    /// SQLite returned Done and the standalone partial file was durably flushed.
    Copied,
    /// Integrity verification and durable final-name publication both finished.
    Complete,
    Abandoned,
    /// No database or WAL existed when the startup owner inspected this root.
    NotRequired,
}
impl Phase {
    pub const fn finished(self) -> bool {
        matches!(self, Self::Complete | Self::Abandoned | Self::NotRequired)
    }
}

/// Failures are terminal. Process interruption is a separate recovery operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Space,
    Deadline,
    WalLimit,
    Snapshot,
    Copy,
    Integrity,
    Publication,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Failed(FailureKind),
    Interrupted,
}

/// One immutable attempt identity with a monotonically advancing local revision.
/// Attempt zero is T0; attempt one is the single permitted post-restart T1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub service: Service,
    pub format: u32,
    pub written_by: String,
    pub previous_format: Option<u32>,
    /// Persisted pre-open file evidence, not a caller-supplied skip-backup flag.
    pub backup_basis: BackupBasis,
    pub upgraded_at_millis: i64,
    pub attempt: u8,
    pub revision: u64,
    pub phase: Phase,
    pub snapshot_at_millis: Option<i64>,
    /// A single basename under the owning data root's upgrade-backups directory.
    pub file_name: Option<String>,
    pub copied_pages: Option<u32>,
    pub end_reason: Option<EndReason>,
    pub reason: Option<String>,
}

pub struct Begin {
    pub service: Service,
    pub written_by: String,
    pub previous_format: Option<u32>,
    pub at_millis: i64,
    /// Observe under exclusive startup ownership, before opening/creating SQLite.
    pub backup_basis: BackupBasis,
}
impl Begin {
    pub fn record(self) -> Result<Record> {
        let record = Record {
            service: self.service,
            format: self.service.format(),
            written_by: self.written_by,
            previous_format: self.previous_format,
            backup_basis: self.backup_basis,
            upgraded_at_millis: self.at_millis,
            attempt: 0,
            revision: 1,
            phase: if self.backup_basis.needs_backup() { Phase::Prepared } else { Phase::NotRequired },
            snapshot_at_millis: None,
            file_name: None,
            copied_pages: None,
            end_reason: None,
            reason: None,
        };
        record.validate()?;
        Ok(record)
    }
}

#[derive(Clone, Debug)]
pub enum Transition {
    /// The executor already holds the actual SQLite read snapshot.
    Pinned { at_millis: i64, file_name: String },
    /// Must follow Backup::step(Done), standalone close and durable file flush.
    Copied { pages: u32 },
    /// Must follow successful integrity_check and durable final-name publication.
    Complete,
    /// The executor released its snapshot. Interrupted work uses Journal::interrupted.
    Failed { kind: FailureKind, detail: String },
}

/// Every change persists through the service's FULL writer and compares the entire
/// expected record. Exact replay returns the same result; no SQL escapes this port.
pub trait Journal: Send + Sync {
    fn current(&self) -> Result<Record>;
    /// At most two records. Historical abandonment does not open the migration gate.
    fn attempts(&self) -> Result<Vec<Record>>;
    fn advance(&self, expected: Record, transition: Transition) -> Result<Record>;
    /// Atomically retires interrupted T0 and reserves a distinct basename for T1.
    /// T0 requires Some(name); T1 requires None and terminates without another retry.
    /// The executor must delete the old partial before releasing migration.
    fn interrupted(&self, expected: Record, retry_file_name: Option<String>) -> Result<Record>;
}
