//! Bounded history reads never ask the host to reconstruct a conversation.
use crate::{
    identity::ObjectId,
    store::{FutureResult, access::AccessContext},
};
pub use fastctx_types::sync::timeline::*;
pub mod changes;
pub mod index;
pub mod pages;
pub mod position;
pub mod query;
pub mod scan;
mod tree;
pub mod update;

pub trait TimelineReader: Send + Sync {
    /// What the installed records say of `conversation`'s scope to `principal`:
    /// the holding period its reads are answered under, and the recorded fact
    /// that ends them, if one does. None when no record of the scope is
    /// installed on this device.
    fn standing(
        &self,
        principal: crate::store::access::Principal,
        conversation: crate::identity::ObjectKey,
    ) -> FutureResult<'_, Option<ScopeStanding>>;
    /// Waits, at most thirty seconds, for the records to say something other
    /// than `known`, and answers what they say then.
    fn standing_beyond(
        &self,
        principal: crate::store::access::Principal,
        conversation: crate::identity::ObjectKey,
        known: ScopeStanding,
    ) -> FutureResult<'_, Option<ScopeStanding>>;
    fn stream(&self, context: AccessContext, request: StreamRead) -> FutureResult<'_, StreamPage>;
    /// Registers a durable root lease for a fresh fence. A stale lease returns
    /// ReanchorNeeded; absent local projection state returns Pending.
    fn timeline(&self, context: AccessContext, request: TimelineRead) -> FutureResult<'_, TimelineResult>;
    fn item(&self, context: AccessContext, fence: TimelineFence, item: ObjectId) -> FutureResult<'_, ItemRead>;
    fn wait(
        &self,
        context: AccessContext,
        conversation: crate::identity::ObjectKey,
        after: crate::cursor::LocalSequence,
    ) -> FutureResult<'_, TimelineWait>;
    fn changes(&self, context: AccessContext, request: TimelineChangesRead) -> FutureResult<'_, TimelineChangesResult>;
    fn content(&self, context: AccessContext, request: TimelineContentRead) -> FutureResult<'_, TimelineContentResult>;
}
