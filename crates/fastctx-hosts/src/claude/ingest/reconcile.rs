//! Restartable rewrite checks over caller-supplied bounded reads. Short anchors
//! make ordinary append cheap; only a contiguous audit proves an old prefix.
use super::checkpoint::{ClaudeCheckpointV1, RewriteReason};
use fastctx_types::{
    file_identity::{FileChangeStamp, FileIdentity},
    sync::cursor::ByteCount,
};
mod prefix;
mod rows;
pub use prefix::{PrefixCheck, advance_prefix};
pub use rows::{RowMatch, match_row};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Missing,
    /// Finish durable work from the validated private row before opening its host.
    Publish,
    /// Consume already retained source bytes before observing its current path.
    Buffered,
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
    if checkpoint.read_ahead.is_some() {
        return Change::Buffered;
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
        Some(false) => Change::Reconcile(RewriteReason::AnchorChanged),
    }
}
