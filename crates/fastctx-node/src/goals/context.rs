//! Named-context seam; an empty block removes it, and the owner of the context handles compaction.

use super::records::Reply;
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;

pub(crate) trait Context: Send + Sync {
    fn available(&self) -> bool;
    /// `fact` says why the block changed, told with it.
    fn set<'a>(&'a self, resident: &'a ResidentId, name: &'a str, text: &'a str, fact: &'a str) -> Reply<'a, ()>;
}

pub(crate) struct Unconnected;
impl Context for Unconnected {
    fn available(&self) -> bool {
        false
    }
    fn set<'a>(&'a self, _: &'a ResidentId, _: &'a str, _: &'a str, _: &'a str) -> Reply<'a, ()> {
        Box::pin(async { Err(Failure::new(super::words::CONTEXT_MISSING)) })
    }
}
