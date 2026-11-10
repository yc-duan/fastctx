//! The node's timeline reads. Admission is the conversation history's own: the
//! user's voice, and a conversation this World's logins see here. The engines
//! then answer as the World, the only principal under its alone hub.
use std::sync::Arc;

use fastctx_sync::{
    cursor::Revision,
    store::{
        Error,
        access::{AccessContext, Principal},
    },
    timeline::{
        TimelineChangesRead, TimelineContentRead, TimelineCoverage, TimelineRead, TimelineReader, TimelineWait,
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
    if !seen::resident(node, Looker::Here(&Acting::Here), call.resident()) {
        return Err(Failure::refused("this account cannot use that conversation's history"));
    }
    let Some(foundation) = node.foundation.as_ref() else {
        return Ok(TimelineReply::Unavailable);
    };
    let Some(hosted) = foundation.hosted(node, call.resident()) else {
        return Ok(TimelineReply::Unavailable);
    };
    // Scopes under the alone hub keep the ACL revision they were opened with.
    let context = AccessContext {
        principal: Principal::LocalService,
        scope: hosted.timeline.scope.clone(),
        observed_acl: Revision(1),
    };
    let timelines = foundation.timelines();
    let answered = match call {
        TimelineCall::Read { fence, anchor, max_items, max_bytes, .. } => {
            let read = TimelineRead { conversation: hosted.timeline, fence, anchor, max_items, max_bytes };
            timelines.timeline(context, read).await.map(|result| TimelineReply::Read {
                result,
                coverage: TimelineCoverage::Incomplete { reason: "timeline coverage has not been certified".into() },
            })
        }
        TimelineCall::Item { fence, item, .. } => {
            if fence.conversation != hosted.timeline {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            timelines.item(context, fence, item).await.map(|read| TimelineReply::Item { read })
        }
        TimelineCall::Wait { after, .. } => {
            timelines.wait(context, hosted.timeline, after).await.map(|result| match result {
                TimelineWait::Moved { local } => TimelineReply::Moved { local },
                TimelineWait::Unchanged => TimelineReply::Unchanged,
            })
        }
        TimelineCall::Changes { from, to, after, max_items, max_bytes, .. } => {
            if from.conversation != hosted.timeline
                || to.as_ref().is_some_and(|fence| fence.conversation != hosted.timeline)
            {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            let read = TimelineChangesRead { from: *from, to: to.map(|to| *to), after, max_items, max_bytes };
            timelines.changes(context, read).await.map(|result| TimelineReply::Changes { result })
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
        TimelineCall::Release { fence, .. } => {
            if fence.conversation != hosted.timeline {
                return Err(Failure::refused("the fence belongs to another conversation"));
            }
            timelines.release(context, fence).await.map(|()| TimelineReply::Released)
        }
    };
    answered.map_err(failure)
}

fn failure(error: Error) -> Failure {
    match error {
        Error::Busy | Error::Full => {
            Failure::not_run("the conversation's timeline is busy; try again").capacity(CapacityReason::Pending)
        }
        Error::Denied(reason) => Failure::refused(reason),
        Error::Revoked => Failure::refused("the conversation was deleted"),
        other => Failure::new(format!("reading the conversation's timeline: {other}")),
    }
}
