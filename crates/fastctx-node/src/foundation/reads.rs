//! The node's timeline reads. Admission is the conversation history's own: the
//! user's voice, and a conversation this World's logins see here. The engines
//! then answer as the World, the only principal under its alone hub.
//!
//! What ends a reader's reading is said only from a record: the node's own
//! record that the conversation was deleted, or what the scope's installed
//! record says (`TimelineReply::Closed`). An error, a missing row and a scope
//! not yet installed say nothing of the conversation.
use std::sync::Arc;

use fastctx_sync::{
    cursor::{HoldingPeriod, Revision},
    identity::ObjectKey,
    store::{
        Error,
        access::{AccessContext, Principal},
    },
    timeline::{
        ScopeStanding, TimelineChangesRead, TimelineContentRead, TimelineCoverage, TimelineRead, TimelineReader,
        TimelineWait,
    },
};
use fastctx_types::{
    access::Acting,
    ipc::{CapacityReason, Failure},
    timeline_reads::{TimelineCall, TimelineReply},
};

use crate::{
    Node,
    admission::seen::{self, Looker},
};

pub(crate) async fn call(node: &Arc<Node>, call: TimelineCall) -> Result<TimelineReply, Failure> {
    let resident = call.resident();
    if node.residents.about(resident).is_none() {
        return Err(crate::forget::gone(node, resident));
    }
    if !seen::resident(node, Looker::Here(&Acting::Here), resident) {
        return Err(Failure::refused("this account cannot use that conversation's history"));
    }
    let Some(foundation) = node.foundation.as_ref() else {
        return Ok(TimelineReply::Unavailable);
    };
    let Some(hosted) = foundation.hosted(node, resident) else {
        return Ok(TimelineReply::Unavailable);
    };
    let timelines = foundation.timelines();
    // A hold is given back whatever the scope's record says by now: releasing
    // asks for no right to read, so the record is not asked first.
    if let TimelineCall::Release { fence, .. } = &call {
        if fence.conversation != hosted.timeline {
            return Err(Failure::refused("the fence belongs to another conversation"));
        }
        let context = AccessContext {
            principal: Principal::LocalService,
            scope: hosted.timeline.scope.clone(),
            observed_acl: Revision(1),
        };
        return timelines.release(context, fence.clone()).await.map(|()| TimelineReply::Released).map_err(failure);
    }
    let recorded = timelines.standing(Principal::LocalService, hosted.timeline.clone()).await.map_err(failure)?;
    let recorded = match (recorded, &call) {
        // The reader was already told this ending: the wait holds until the
        // records say something else, as it holds for a head that has not moved.
        (Some(known @ ScopeStanding { end: Some(_), .. }), TimelineCall::Wait { closed: Some(told), .. })
            if known.period == *told =>
        {
            timelines.standing_beyond(Principal::LocalService, hosted.timeline.clone(), known).await.map_err(failure)?
        }
        (recorded, _) => recorded,
    };
    let period = match answer(recorded) {
        Standing::Open(period) => period,
        Standing::Answered(reply) => return Ok(reply),
    };
    let context = AccessContext {
        principal: Principal::LocalService,
        scope: hosted.timeline.scope.clone(),
        observed_acl: period.acl_revision,
    };
    let waiting = matches!(call, TimelineCall::Wait { .. });
    let under = period.clone();
    let answered = match call {
        TimelineCall::Read { fence, anchor, max_items, max_bytes, .. } => {
            let read = TimelineRead { conversation: hosted.timeline.clone(), fence, anchor, max_items, max_bytes };
            timelines.timeline(context, read).await.map(|result| TimelineReply::Read {
                result,
                coverage: TimelineCoverage::Incomplete { reason: "timeline coverage has not been certified".into() },
                period,
            })
        }
        TimelineCall::Item { fence, item, .. } => {
            if fence.conversation != hosted.timeline {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            timelines.item(context, fence, item).await.map(|read| TimelineReply::Item { read })
        }
        TimelineCall::Wait { after, .. } => {
            timelines.wait(context, hosted.timeline.clone(), after).await.map(|result| match result {
                TimelineWait::Moved { local } => TimelineReply::Moved { local, period },
                TimelineWait::Unchanged => TimelineReply::Unchanged { period },
            })
        }
        TimelineCall::Changes { from, to, after, max_items, max_bytes, .. } => {
            if from.conversation != hosted.timeline
                || to.as_ref().is_some_and(|fence| fence.conversation != hosted.timeline)
            {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            let read = TimelineChangesRead { from: *from, to: to.map(|to| *to), after, max_items, max_bytes };
            timelines.changes(context, read).await.map(|result| TimelineReply::Changes { result, period })
        }
        TimelineCall::Content { fence, item, content, range, path, .. } => {
            if fence.conversation != hosted.timeline {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            timelines
                .content(context, TimelineContentRead { fence: *fence, item, content, range, path })
                .await
                .map(|read| TimelineReply::Content { read })
        }
        // Answered above, before the scope's record was asked.
        TimelineCall::Release { fence, .. } => {
            if fence.conversation != hosted.timeline {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            timelines.release(context, fence).await.map(|()| TimelineReply::Released)
        }
    };
    match answered {
        Ok(reply) => Ok(reply),
        // The scope's record may have moved under the read. What it says now is
        // the answer; the error alone does not say the conversation is gone.
        Err(error @ (Error::Revoked | Error::Denied(_) | Error::Busy)) => {
            match standing(timelines, &hosted.timeline).await? {
                Standing::Answered(reply) => Ok(reply),
                // A wait the period ended under is over, though no head moved.
                Standing::Open(period) if waiting && period != under => Ok(TimelineReply::Unchanged { period }),
                Standing::Open(_) => Err(failure(error)),
            }
        }
        Err(error) => Err(failure(error)),
    }
}

enum Standing {
    /// The reader may read, under this holding period.
    Open(HoldingPeriod),
    /// The records already answer the call: nothing is installed here, or what
    /// is installed ends the reading.
    Answered(TimelineReply),
}

fn answer(recorded: Option<ScopeStanding>) -> Standing {
    match recorded {
        None => Standing::Answered(TimelineReply::Unavailable),
        Some(ScopeStanding { period, end: Some(end) }) => Standing::Answered(TimelineReply::Closed { period, end }),
        Some(ScopeStanding { period, end: None }) => Standing::Open(period),
    }
}

async fn standing(timelines: &dyn TimelineReader, conversation: &ObjectKey) -> Result<Standing, Failure> {
    timelines.standing(Principal::LocalService, conversation.clone()).await.map(answer).map_err(failure)
}

fn failure(error: Error) -> Failure {
    match error {
        Error::Busy | Error::Full => {
            Failure::not_run("the conversation's timeline is busy; try again").capacity(CapacityReason::Pending)
        }
        Error::Denied(reason) => Failure::refused(reason),
        Error::Revoked => Failure::not_run("the conversation's scope changed under the read; try again"),
        Error::Unavailable(reason) => {
            Failure::not_run(format!("the conversation's timeline cannot be read now: {reason}"))
        }
        other => Failure::new(format!("reading the conversation's timeline: {other}")),
    }
}
