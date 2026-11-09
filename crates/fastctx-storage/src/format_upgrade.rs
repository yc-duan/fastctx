//! Local format-upgrade accounting, independent of account scopes and replication.
//!
//! The owning database identifies the data root; its identity moves with a copied
//! or relocated database. Only startup and upgrade composition receive a journal.

use crate::{Error, Result};

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
    /// No services have been released by this attempt; startup can pin T0 again.
    Prepared,
    /// The snapshot was pinned and this fact committed before services may write.
    Copying,
    /// SQLite returned Done and the standalone partial file was durably flushed.
    Copied,
    /// Integrity verification and durable final-name publication both finished.
    Complete,
    Abandoned,
    /// A new database had no previous content requiring an upgrade backup.
    NotRequired,
}
impl Phase {
    pub const fn finished(self) -> bool {
        matches!(self, Self::Complete | Self::Abandoned | Self::NotRequired)
    }
}

/// One immutable attempt identity with a monotonically advancing local revision.
/// Attempt zero is T0; attempt one is the single permitted post-restart T1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub service: Service,
    pub format: u32,
    pub written_by: String,
    pub previous_format: Option<u32>,
    pub upgraded_at_millis: i64,
    pub attempt: u8,
    pub revision: u64,
    pub phase: Phase,
    pub snapshot_at_millis: Option<i64>,
    /// A single basename under the owning data root's upgrade-backups directory.
    pub file_name: Option<String>,
    pub copied_pages: Option<u32>,
    pub reason: Option<String>,
}

pub struct Begin {
    pub service: Service,
    pub written_by: String,
    pub previous_format: Option<u32>,
    pub at_millis: i64,
    pub needs_backup: bool,
}
impl Begin {
    pub fn record(self) -> Result<Record> {
        let record = Record {
            service: self.service,
            format: self.service.format(),
            written_by: self.written_by,
            previous_format: self.previous_format,
            upgraded_at_millis: self.at_millis,
            attempt: 0,
            revision: 1,
            phase: if self.needs_backup { Phase::Prepared } else { Phase::NotRequired },
            snapshot_at_millis: None,
            file_name: None,
            copied_pages: None,
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
    /// The executor has released its read snapshot and stopped touching the file.
    Abandon { reason: String },
}

impl Record {
    pub fn validate(&self) -> Result<()> {
        let valid_name = self.file_name.as_ref().is_none_or(|name| {
            !name.is_empty()
                && name.len() <= 192
                && name.ends_with(".sqlite3")
                && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
                && !name.starts_with('.')
        });
        if self.format != self.service.format()
            || self.previous_format.is_some_and(|format| format == 0 || format > self.format)
            || self.written_by.is_empty()
            || self.written_by.len() > 128
            || self.upgraded_at_millis < 0
            || self.snapshot_at_millis.is_some_and(|time| time < 0)
            || self.attempt > 1
            || self.revision == 0
            || !valid_name
            || self.reason.as_ref().is_some_and(|reason| reason.is_empty() || reason.len() > 4096)
        {
            return Err(Error::new("invalid format-upgrade record"));
        }
        let snapshot = self.snapshot_at_millis.is_some() && self.file_name.is_some();
        let clean = self.snapshot_at_millis.is_none() && self.file_name.is_none() && self.copied_pages.is_none();
        let valid_phase = match self.phase {
            Phase::Prepared | Phase::NotRequired => clean && self.reason.is_none(),
            Phase::Copying => snapshot && self.copied_pages.is_none() && self.reason.is_none(),
            Phase::Copied | Phase::Complete => {
                snapshot && self.copied_pages.is_some_and(|pages| pages > 0) && self.reason.is_none()
            }
            Phase::Abandoned => (clean || snapshot) && self.reason.is_some(),
        };
        if !valid_phase || self.attempt == 1 && self.phase == Phase::NotRequired {
            return Err(Error::new("format-upgrade phase disagrees with its durable evidence"));
        }
        Ok(())
    }

    pub fn advance(&self, transition: &Transition) -> Result<Self> {
        self.validate()?;
        let mut next = self.clone();
        next.revision = next.revision.checked_add(1).ok_or_else(|| Error::new("upgrade revision exhausted"))?;
        match (self.phase, transition) {
            (Phase::Prepared, Transition::Pinned { at_millis, file_name }) => {
                next.phase = Phase::Copying;
                next.snapshot_at_millis = Some(*at_millis);
                next.file_name = Some(file_name.clone());
            }
            (Phase::Copying, Transition::Copied { pages }) => {
                next.phase = Phase::Copied;
                next.copied_pages = Some(*pages);
            }
            (Phase::Copied, Transition::Complete) => next.phase = Phase::Complete,
            (Phase::Prepared | Phase::Copying | Phase::Copied, Transition::Abandon { reason }) => {
                next.phase = Phase::Abandoned;
                next.reason = Some(reason.clone());
            }
            _ => return Err(Error::new("invalid format-upgrade phase transition")),
        }
        next.validate()?;
        Ok(next)
    }

    /// Only interruption of the open-service T0 copying phase permits a new T1.
    /// Ordinary failures are terminal; an interrupted T1 is also terminal.
    pub fn interrupted(&self) -> Result<(Self, Option<Self>)> {
        let abandoned = self.advance(&Transition::Abandon {
            reason: "process ended before the pinned snapshot finished copying".into(),
        })?;
        if self.phase != Phase::Copying {
            return Err(Error::new("only an interrupted copying phase loses its pinned snapshot"));
        }
        let retry = (self.attempt == 0).then(|| Self {
            attempt: 1,
            revision: 1,
            phase: Phase::Prepared,
            snapshot_at_millis: None,
            file_name: None,
            copied_pages: None,
            reason: None,
            ..self.clone()
        });
        Ok((abandoned, retry))
    }
}

/// The implementation persists every change through the service's FULL writer.
/// It compares the entire expected record; exact replay returns the same result.
/// This trait conveys upgrade accounting only, never SQL or arbitrary row writes.
pub trait Journal: Send + Sync {
    fn current(&self) -> Result<Record>;
    /// At most two records of this format, including abandoned T0's partial name.
    fn attempts(&self) -> Result<Vec<Record>>;
    fn advance(&self, expected: Record, transition: Transition) -> Result<Record>;
    /// Atomically retires interrupted T0 and prepares its one T1, or retires T1.
    /// The executor must remove the old partial file before releasing migration.
    fn interrupted(&self, expected: Record) -> Result<Record>;
}
