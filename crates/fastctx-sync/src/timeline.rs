//! Bounded history reads never ask the host to reconstruct a conversation.
use crate::{
    identity::ObjectId,
    store::{FutureResult, access::AccessContext},
};
pub use fastctx_types::sync::timeline::*;

pub trait TimelineReader: Send + Sync {
    fn stream(&self, context: AccessContext, request: StreamRead) -> FutureResult<'_, StreamPage>;
    fn timeline(&self, context: AccessContext, request: TimelineRead) -> FutureResult<'_, TimelinePage>;
    fn item(
        &self,
        context: AccessContext,
        fence: TimelineFence,
        item: ObjectId,
    ) -> FutureResult<'_, Option<TimelineItem>>;
}
