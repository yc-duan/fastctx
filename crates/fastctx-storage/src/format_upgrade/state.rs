//! Phase transitions distinguish failed physical work from loss of a live snapshot.
use super::{EndReason, FailureKind, Phase, Record, Transition, names};
use crate::{Error, Result};

impl Record {
    pub fn validate(&self) -> Result<()> {
        super::validate_written_by(&self.written_by)?;
        if self.format != self.service.format()
            || self.previous_format.is_some_and(|format| format == 0 || format >= self.format)
            || self.upgraded_at_millis < 0
            || self.snapshot_at_millis.is_some_and(|time| time < 0)
            || self.attempt > 1
            || self.revision == 0
            || self.file_name.as_ref().is_some_and(|name| !names::valid(name))
            || self.copied_pages == Some(0)
            || self.reason.as_ref().is_some_and(|reason| reason.is_empty() || reason.len() > 4096)
            || self.reason.is_some() != self.end_reason.is_some()
        {
            return Err(Error::new("invalid format-upgrade record"));
        }
        let snapshot = self.snapshot_at_millis.is_some() && self.file_name.is_some();
        let unpinned = self.snapshot_at_millis.is_none() && self.copied_pages.is_none();
        let reserved = self.file_name.is_some() == (self.attempt == 1);
        let no_reason = self.reason.is_none();
        let valid_phase = match self.phase {
            Phase::Prepared => unpinned && reserved && no_reason,
            Phase::NotRequired => unpinned && self.file_name.is_none() && no_reason && self.attempt == 0,
            Phase::Copying => snapshot && self.copied_pages.is_none() && no_reason,
            Phase::Copied | Phase::Complete => snapshot && self.copied_pages.is_some() && no_reason,
            Phase::Abandoned => (unpinned && reserved || snapshot) && self.reason.is_some(),
        };
        if !valid_phase
            || self.backup_basis.needs_backup() == (self.phase == Phase::NotRequired)
            || !self.backup_basis.needs_backup() && self.previous_format.is_some()
            || self.end_reason == Some(EndReason::Interrupted) && (!snapshot || self.copied_pages.is_some())
        {
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
                if self.file_name.as_ref().is_some_and(|reserved| reserved != file_name) {
                    return Err(Error::new("retry publication must use its reserved distinct basename"));
                }
                next.phase = Phase::Copying;
                next.snapshot_at_millis = Some(*at_millis);
                next.file_name = Some(file_name.clone());
            }
            (Phase::Copying, Transition::Copied { pages }) => {
                next.phase = Phase::Copied;
                next.copied_pages = Some(*pages);
            }
            (Phase::Copied, Transition::Complete) => next.phase = Phase::Complete,
            (_, Transition::Failed { kind, detail }) if failure_applies(self.phase, *kind) => {
                next.phase = Phase::Abandoned;
                next.end_reason = Some(EndReason::Failed(*kind));
                next.reason = Some(detail.clone());
            }
            _ => return Err(Error::new("invalid format-upgrade phase transition")),
        }
        next.validate()?;
        Ok(next)
    }

    /// Recovery cannot silently treat snapshot loss as an ordinary terminal failure.
    pub fn interrupted(&self, retry_file_name: Option<String>) -> Result<(Self, Option<Self>)> {
        self.validate()?;
        if self.phase != Phase::Copying {
            return Err(Error::new("only an interrupted copying phase loses its pinned snapshot"));
        }
        let retry = match (self.attempt, retry_file_name) {
            (0, Some(name)) => {
                if !names::valid(&name) || self.file_name.as_ref().is_some_and(|old| old.eq_ignore_ascii_case(&name)) {
                    return Err(Error::new("T1 needs a valid basename distinct from T0 on Windows"));
                }
                Some(Self {
                    attempt: 1,
                    revision: 1,
                    phase: Phase::Prepared,
                    snapshot_at_millis: None,
                    file_name: Some(name),
                    copied_pages: None,
                    end_reason: None,
                    reason: None,
                    ..self.clone()
                })
            }
            (1, None) => None,
            _ => return Err(Error::new("T0 must reserve T1; T1 cannot reserve another retry")),
        };
        let mut abandoned = self.clone();
        abandoned.revision =
            abandoned.revision.checked_add(1).ok_or_else(|| Error::new("upgrade revision exhausted"))?;
        abandoned.phase = Phase::Abandoned;
        abandoned.end_reason = Some(EndReason::Interrupted);
        abandoned.reason = Some("process ended before the pinned snapshot finished copying".into());
        abandoned.validate()?;
        if let Some(record) = &retry {
            record.validate()?;
        }
        Ok((abandoned, retry))
    }
}

fn failure_applies(phase: Phase, failure: FailureKind) -> bool {
    match failure {
        FailureKind::Space => matches!(phase, Phase::Prepared | Phase::Copying),
        FailureKind::Deadline => matches!(phase, Phase::Prepared | Phase::Copying | Phase::Copied),
        FailureKind::WalLimit | FailureKind::Copy => phase == Phase::Copying,
        FailureKind::Snapshot => phase == Phase::Prepared,
        FailureKind::Integrity | FailureKind::Publication => phase == Phase::Copied,
    }
}
