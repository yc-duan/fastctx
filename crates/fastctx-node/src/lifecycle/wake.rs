//! One admission point for bringing a host back, regardless of work source.

use crate::Node;
use fastctx_types::{NoticeId, ResidentId, ipc::Failure, residents::Notice, session::Speaker};
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Source {
    User,
    Resident,
    Fact,
    JobEnded,
    Answer,
    Control,
    Management,
    View,
    HostInterface,
}

impl Source {
    pub(crate) fn of(notice: &Notice) -> Self {
        match notice {
            Notice::Words { from: Speaker::User { .. }, .. } | Notice::Attached { .. } => Self::User,
            Notice::Words { .. } => Self::Resident,
            Notice::Answer { .. } => Self::Answer,
            Notice::JobEnded { .. } => Self::JobEnded,
            _ => Self::Fact,
        }
    }
}

pub(crate) struct WakeFailure {
    pub failure: Failure,
    pub attempted: bool,
}

impl From<Failure> for WakeFailure {
    fn from(failure: Failure) -> Self {
        Self { failure, attempted: false }
    }
}

/// The caller owns the action gate and a shared management gate (or an
/// exclusive gate during legacy management). No admission order or concurrency
/// limit is imposed across conversations. A prompt is supplied only by an
/// adapter whose resume protocol accepts it atomically with startup.
pub(crate) async fn wake(
    node: &Arc<Node>,
    id: &ResidentId,
    source: Source,
    prompt: Option<(NoticeId, String)>,
) -> Result<Option<String>, WakeFailure> {
    node.residents.active(id)?;
    let (about, _) = node.residents.about(id).ok_or_else(crate::authorization::no_subject)?;
    crate::homes::available(node, &about)?;
    crate::assists::may_turn(node, id, &about)?;
    crate::town::access::current(node, id)?;
    let adapter = super::adapters::Adapter::for_conversation(&about)
        .ok_or_else(|| Failure::new("this host has no supported lifecycle entrance"))?;
    node.log.line(format_args!("lifecycle {id}: host requested by {source:?}"));
    let carries_words = prompt.is_some();
    let missing = adapter.wake(node, &about, prompt).await?;
    if !carries_words {
        super::observed(node, id, true, &format!("requested by {source:?}"));
    }
    Ok(missing)
}
