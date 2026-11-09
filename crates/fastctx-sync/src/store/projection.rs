//! Projection snapshots and subscriptions share the writer's commit watermark.
use super::{FutureResult, PageLimit};
use crate::{
    cursor::{LocalSequence, Revision},
    identity::ObjectId,
};

#[derive(Clone, Debug)]
pub struct ProjectionRecord {
    pub revision: Revision,
    /// Source changes incorporated into this projection, not its commit order.
    pub applied: LocalSequence,
    /// Assigned with the row, roots and durable notification in one transaction.
    pub local: LocalSequence,
    pub body: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ProjectionNotice {
    pub object: ObjectId,
    pub revision: Revision,
    pub applied: LocalSequence,
    pub local: LocalSequence,
}
/// Subscribe first, read the snapshot second, then resume strictly after its
/// `local` fence. Notifications come from durable rows; a wake-up is not an event.
pub trait ProjectionSubscription: Send {
    fn next(&mut self, limit: PageLimit) -> FutureResult<'_, Vec<ProjectionNotice>>;
}
