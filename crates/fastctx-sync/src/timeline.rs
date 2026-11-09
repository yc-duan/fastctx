//! Bounded history reads never ask the host to reconstruct a conversation.
use crate::{
    identity::ObjectId,
    store::{FutureResult, access::AccessContext},
};
pub use fastctx_types::sync::timeline::*;
pub mod index;
pub mod pages;
pub mod position;
pub mod query;
mod tree;
pub mod update;

pub trait TimelineReader: Send + Sync {
    fn stream(&self, context: AccessContext, request: StreamRead) -> FutureResult<'_, StreamPage>;
    /// Registers a durable root lease for a fresh fence. A stale lease returns
    /// ReanchorNeeded; absent local projection state returns Pending.
    fn timeline(&self, context: AccessContext, request: TimelineRead) -> FutureResult<'_, TimelineResult>;
    fn item(&self, context: AccessContext, fence: TimelineFence, item: ObjectId) -> FutureResult<'_, ItemRead>;
}
