//! Check a reviewer's home before freezing potentially large local evidence.

use super::super::reviewer::{Answer, Readiness};
use crate::Node;
use fastctx_types::NoticeId;
use fastctx_types::goals::Goal;
use fastctx_types::goals::runtime::{ReviewCause, Work};
use std::sync::Arc;
use std::time::Duration;

type Observed = Option<(jiff::Timestamp, NoticeId)>;

pub(super) async fn run(node: &Arc<Node>, goal: &Goal, work: &Work, cause: ReviewCause) -> (Observed, Answer) {
    match tokio::time::timeout(Duration::from_secs(60), node.goals.reviewer.readiness(&goal.account)).await {
        Ok(Ok(Readiness::Ready)) => {}
        Ok(Ok(Readiness::Offline { reason })) => return (None, Answer::Offline { reason }),
        Ok(Ok(Readiness::Absent { reason })) => return (None, Answer::Absent { reason }),
        Ok(Err(error)) => return (None, Answer::Failed { reason: error.message }),
        Err(_) => {
            return (
                None,
                Answer::Failed { reason: "The goal reviewer did not report availability within one minute.".into() },
            );
        }
    }
    if !super::requests(node, &goal.resident.resident, work.id.as_str()) {
        return (None, Answer::Failed { reason: "The goal no longer requests this review.".into() });
    }
    let material = match super::materials::build(node, goal, work, cause).await {
        Ok(material) => material,
        Err(error) => return (None, Answer::Failed { reason: error.message }),
    };
    if !super::requests(node, &goal.resident.resident, work.id.as_str()) {
        return (None, Answer::Failed { reason: "The goal no longer requests this review.".into() });
    }
    let observed = material
        .world_actions
        .as_ref()
        .and_then(|facts| facts.snapshot.clone().map(|snapshot| (facts.through, snapshot)));
    let judged =
        tokio::time::timeout(Duration::from_secs(10 * 60), node.goals.reviewer.review(work.id.as_str(), &material))
            .await;
    let answer = match judged {
        Ok(Ok(answer)) => answer.validated(),
        Ok(Err(error)) => Answer::Failed { reason: error.message },
        Err(_) => Answer::Failed { reason: "The goal reviewer did not answer within ten minutes.".into() },
    };
    (observed, answer)
}
