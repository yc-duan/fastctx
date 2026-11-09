//! Restartable rewrite checks over caller-supplied bounded reads. Short anchors
//! make ordinary append cheap; only a contiguous audit proves an old prefix.
use super::checkpoint::{Anchor, ClaudeCheckpointV1, Prefix, RewriteReason};
use fastctx_types::{
    file_identity::{FileChangeStamp, FileIdentity},
    sync::{cursor::ByteCount, identity::Digest},
};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Missing,
    /// Finish durable work from the validated private row before opening its host.
    Publish,
    Unchanged,
    Append {
        from: ByteCount,
    },
    CheckAnchors,
    Reconcile(RewriteReason),
}
/// `anchors_match` is supplied only after checking the persisted short head/tail
/// windows. An unchanged stat never advances the independent durable audit.
pub fn classify(
    checkpoint: &ClaudeCheckpointV1,
    sample: Option<(&FileIdentity, &FileChangeStamp)>,
    anchors_match: Option<bool>,
    host_rewritten: bool,
) -> Change {
    if matches!(&checkpoint.partial, Some(super::checkpoint::Partial::Publishing { .. })) {
        return Change::Publish;
    }
    let Some((identity, stamp)) = sample else {
        return Change::Missing;
    };
    if identity != &checkpoint.file_identity {
        return Change::Reconcile(RewriteReason::Replaced);
    }
    if stamp.size < checkpoint.read_offset {
        return Change::Reconcile(RewriteReason::Truncated);
    }
    if host_rewritten {
        return Change::Reconcile(RewriteReason::HostRewritten);
    }
    if anchors_match == Some(false) {
        return Change::Reconcile(RewriteReason::AnchorChanged);
    }
    if stamp == &checkpoint.observed_stamp {
        return if stamp.size > checkpoint.read_offset {
            Change::Append { from: checkpoint.read_offset }
        } else {
            Change::Unchanged
        };
    }
    match anchors_match {
        None => Change::CheckAnchors,
        Some(true) if stamp.size > checkpoint.read_offset => Change::Append { from: checkpoint.read_offset },
        Some(true) => Change::Reconcile(RewriteReason::AnchorChanged),
        Some(false) => unreachable!(),
    }
}
/// Rechecks one saved range, never constructs a hash for bytes not actually read.
/// The caller uses its registered anchor index to supply pages in ascending order.
pub fn advance_prefix(prefix: &Prefix, anchor: &Anchor, bytes: &[u8]) -> io::Result<PrefixCheck> {
    if anchor.offset != prefix.checked_through
        || anchor.bytes == 0
        || anchor.bytes > super::lexical::WINDOW_BYTES as u32
        || bytes.len() != anchor.bytes as usize
    {
        return Err(io::Error::other("prefix audit requires the next complete bounded anchor"));
    }
    let through = anchor
        .offset
        .0
        .checked_add(u64::from(anchor.bytes))
        .ok_or_else(|| io::Error::other("anchor range overflow"))?;
    if anchor.row_boundary.0 > through || anchor.row_boundary < prefix.row_boundary {
        return Err(io::Error::other("anchor row boundary moved outside its checked prefix"));
    }
    if Digest::of(bytes) != anchor.digest {
        return Ok(PrefixCheck::Changed { reparse_from: prefix.row_boundary });
    }
    Ok(PrefixCheck::Matched(Prefix {
        checked_through: ByteCount(through),
        row_boundary: anchor.row_boundary,
        anchor: Some(anchor.clone()),
    }))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefixCheck {
    Matched(Prefix),
    Changed { reparse_from: ByteCount },
}
/// A UUID match is only a candidate. Reuse requires the same complete row bytes;
/// a changed body with the same UUID is a revision of the same native item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowMatch {
    Unchanged,
    Revision,
    Different,
}
pub fn match_row(old_uuid: &str, new_uuid: &str, old: &Digest, new: &Digest) -> RowMatch {
    if old_uuid != new_uuid || old_uuid.is_empty() {
        RowMatch::Different
    } else if old == new {
        RowMatch::Unchanged
    } else {
        RowMatch::Revision
    }
}
