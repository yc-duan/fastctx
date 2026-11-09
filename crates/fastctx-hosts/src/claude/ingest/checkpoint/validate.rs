//! Check parser-private state against its outer source coordinates, then enforce
//! the one transition that installs a reconciled file generation.
use super::*;
use std::io;

pub const BLOCKED_REASON_BYTES: usize = 1024;
pub struct CheckpointContext<'a> {
    pub source: &'a SourceId,
    pub generation: &'a HostGeneration,
    pub revision: Revision,
    pub committed: ByteCount,
}
impl Anchor {
    pub fn validate(&self, through: ByteCount) -> io::Result<()> {
        let end = self.offset.0.checked_add(u64::from(self.bytes));
        if self.bytes == 0
            || self.bytes as usize > crate::claude::ingest::lexical::WINDOW_BYTES
            || end.is_none_or(|end| end > through.0 || self.row_boundary.0 > end)
        {
            return Err(io::Error::other("anchor exceeds its checked bounded range"));
        }
        Ok(())
    }
}
impl Prefix {
    pub fn validate(&self, read_offset: ByteCount) -> io::Result<()> {
        if self.row_boundary > self.checked_through || self.checked_through > read_offset {
            return Err(io::Error::other("reconciled prefix exceeds its read boundary"));
        }
        if let Some(anchor) = &self.anchor {
            anchor.validate(self.checked_through)?;
            if anchor.offset.0 + u64::from(anchor.bytes) != self.checked_through.0
                || anchor.row_boundary != self.row_boundary
            {
                return Err(io::Error::other("prefix does not end at its last verified anchor"));
            }
        } else if self.checked_through.0 != 0 || self.row_boundary.0 != 0 {
            return Err(io::Error::other("nonempty prefix has no verified anchor"));
        }
        Ok(())
    }
}
impl ClaudeCheckpointV1 {
    pub fn validate(&self, context: &CheckpointContext<'_>) -> io::Result<()> {
        let committed = context.committed;
        if self.version != PARSER_VERSION || context.revision.0 == 0 {
            return Err(io::Error::other("Claude parser version or checkpoint revision is invalid"));
        }
        if self.read_offset > self.observed_stamp.size
            || !matches!(self.phase, Phase::Reconciling { .. }) && committed > self.read_offset
        {
            return Err(io::Error::other("Claude checkpoint offsets are inconsistent"));
        }
        if let Some(ahead) = &self.read_ahead {
            ahead.validate(self.read_offset, self.observed_stamp.size)?;
        }
        match &self.phase {
            Phase::Reconciling { previous, generation, prefix, old_branch, new_branch, .. } => {
                if previous.0 == 0
                    || *previous >= context.revision
                    || generation == context.generation
                    || old_branch == new_branch
                    || self.partial.is_some()
                    || self.read_ahead.is_some()
                    || self.read_offset != prefix.checked_through
                {
                    return Err(io::Error::other(
                        "reconciliation must retain the old outer generation and a verified prefix",
                    ));
                }
                prefix.validate(self.read_offset)?;
            }
            Phase::Blocked { reason, .. } if reason.is_empty() || reason.len() > BLOCKED_REASON_BYTES => {
                return Err(io::Error::other("blocked reason exceeds its UTF-8 byte bound"));
            }
            _ => {}
        }
        match &self.partial {
            Some(partial) => {
                if partial.boundaries() != (committed, self.read_offset)
                    || partial.coordinates() != (context.source, context.generation)
                {
                    return Err(io::Error::other(
                        "partial row differs from its source, generation or committed boundary",
                    ));
                }
            }
            None if self.read_offset != committed && !matches!(self.phase, Phase::Reconciling { .. }) => {
                return Err(io::Error::other("Claude checkpoint lost its partial line"));
            }
            _ => {}
        }
        if self.audit_cursor.through > self.audit_cursor.observation.size {
            return Err(io::Error::other("Claude audit cursor exceeds its observation"));
        }
        if let Some(Partial::Reading { line }) = &self.partial
            && line.document.key.line_start.0.checked_add(line.document.bodies.raw.bytes.0) != Some(line.read_offset.0)
        {
            return Err(io::Error::other("partial row span differs from its preserved bytes"));
        }
        if let Some(Partial::Publishing { row, .. }) = &self.partial {
            let bodies = match &row.content {
                Completion::Valid(document) => &document.bodies,
                Completion::Invalid { bodies, .. } => bodies,
            };
            if row.start.0.checked_add(bodies.raw.bytes.0).and_then(|end| end.checked_add(u64::from(!row.sealed)))
                != Some(row.end.0)
            {
                return Err(io::Error::other("validated row span differs from its preserved bytes"));
            }
        }
        for content in self.roots() {
            content.validate().map_err(io::Error::other)?;
        }
        Ok(())
    }
    /// A generation changes only after the previous persisted reconciliation has
    /// established the exact row boundary from which following will restart.
    /// Issued but cancelled steps consume revisions without moving the persisted
    /// checkpoint, so successful checkpoint revisions need not be consecutive.
    pub fn validate_transition(
        &self,
        next: &CheckpointContext<'_>,
        previous: Option<(&Self, &CheckpointContext<'_>)>,
    ) -> io::Result<()> {
        self.validate(next)?;
        let Some((old, outer)) = previous else {
            if matches!(self.phase, Phase::Reconciling { .. }) {
                return Err(io::Error::other("reconciliation has no previous checkpoint"));
            }
            return Ok(());
        };
        old.validate(outer)?;
        if outer.source != next.source || outer.revision >= next.revision {
            return Err(io::Error::other("checkpoint does not continue its source revision"));
        }
        if outer.generation != next.generation {
            let Phase::Reconciling { generation, prefix, .. } = &old.phase else {
                return Err(io::Error::other("generation changed without persisted reconciliation"));
            };
            if generation != next.generation
                || !matches!(self.phase, Phase::Following)
                || self.partial.is_some()
                || self.read_offset != prefix.row_boundary
                || next.committed != prefix.row_boundary
                || self.file_identity != old.file_identity
                || self.observed_stamp != old.observed_stamp
            {
                return Err(io::Error::other("generation switch differs from its completed consistent prefix"));
            }
        } else if let Phase::Reconciling { previous, generation, prefix, reason, old_branch, new_branch } = &self.phase
        {
            if next.committed != outer.committed {
                return Err(io::Error::other("reconciliation changed the old generation's committed offset"));
            }
            if let Phase::Reconciling {
                previous: p,
                generation: g,
                prefix: f,
                reason: r,
                old_branch: o,
                new_branch: n,
            } = &old.phase
            {
                if *previous == outer.revision && generation != g && prefix.checked_through.0 == 0 {
                    return Ok(());
                }
                if previous != p
                    || generation != g
                    || reason != r
                    || old_branch != o
                    || new_branch != n
                    || prefix.checked_through < f.checked_through
                    || prefix.row_boundary < f.row_boundary
                    || self.file_identity != old.file_identity
                    || self.observed_stamp != old.observed_stamp
                {
                    return Err(io::Error::other("reconciliation attempt changed its pinned input or target"));
                }
            } else if *previous != outer.revision || prefix.checked_through.0 != 0 {
                return Err(io::Error::other(
                    "new reconciliation must begin at the previous revision with an empty audit",
                ));
            }
        } else if matches!(old.phase, Phase::Reconciling { .. }) {
            return Err(io::Error::other("reconciliation completed without installing its new generation"));
        } else if self.file_identity != old.file_identity
            || self.observed_stamp.size < old.observed_stamp.size
            || self.read_offset < old.read_offset
        {
            return Err(io::Error::other("replacement or rollback requires persisted reconciliation"));
        }
        if outer.generation == next.generation
            && !matches!(self.phase, Phase::Reconciling { .. })
            && let Some(ahead) = &old.read_ahead
            && self.read_offset < ahead.end()?
            && self.read_ahead.as_ref().is_none_or(|next| {
                next.start != ahead.start
                    || next.bytes != ahead.bytes
                    || next.content != ahead.content
                    || next.consumed < ahead.consumed
            })
        {
            return Err(io::Error::other("following discarded or replaced unread staged bytes"));
        }
        Ok(())
    }
}
