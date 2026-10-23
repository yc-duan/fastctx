//! The colleague-facing contract of the goal runtime.

use crate::Node;
type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
use fastctx_types::{
    Address, ResidentId,
    ipc::Failure,
    residents::{About, Notice, Origin, Pending},
    session::Speaker,
};
use std::sync::Arc;

/// Goal state belongs to the goal runtime. Opening is idempotent for a
/// conversation and owner; reopening acts only on a blocked goal and deduplicates
/// the triggering delivery. Completion reports and questions are delivered by
/// that runtime, never by the ordinary turn-completion path.
pub(crate) trait ColleagueGoals: Send + Sync {
    fn ready(&self) -> Result<(), Failure>;
    fn open<'a>(
        &'a self,
        node: &'a Arc<Node>,
        resident: &'a ResidentId,
        owner: &'a Address,
        objective: &'a str,
    ) -> BoxFuture<'a, Result<(), Failure>>;
    /// Runs inside the recipient's handoff gate. Commit the state transition here;
    /// any further delivery must be scheduled without reacquiring that gate.
    fn reopen_blocked<'a>(
        &'a self,
        node: &'a Arc<Node>,
        resident: &'a ResidentId,
        delivery: &'a fastctx_types::NoticeId,
    ) -> BoxFuture<'a, Result<(), Failure>>;
    fn state<'a>(
        &'a self,
        node: &'a Arc<Node>,
        resident: &'a ResidentId,
    ) -> BoxFuture<'a, Result<Option<fastctx_types::collab::GoalSummary>, Failure>>;
}

pub(crate) async fn incoming(
    node: &Arc<Node>,
    id: &ResidentId,
    about: &About,
    pending: &Pending,
) -> Result<(), Failure> {
    if let Origin::Colleague { asked_by, .. } = &about.origin
        && pending.readers.contains(asked_by)
        && let Notice::Words { from: Speaker::Resident { address }, .. } = &pending.delivery.notice
        && address == asked_by
    {
        node.colleague_goals.reopen_blocked(node, id, &pending.delivery.id).await?;
    }
    Ok(())
}
