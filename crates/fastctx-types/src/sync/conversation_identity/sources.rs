//! Native parser checkpoints and the conversation's shared reducer have distinct
//! source namespaces. Neither a runtime path nor a producer epoch renames them.
use super::{NativeId, encoding::Encoder};
use crate::sync::identity::{DeviceId, ObjectId, SourceId};

/// The checkpoint source for one claimed Claude JSONL. Child conversations use
/// their own canonical conversation identity and the enclosing native session.
/// A different hosting device has a new local parser source, while its native
/// item identities remain shared. This prevents local HostGeneration seeds from
/// colliding after a canonical producer handover.
pub fn claude_transcript_source(conversation: &ObjectId, device: &DeviceId, session: &NativeId) -> SourceId {
    let mut into = Encoder::new("source");
    into.text("claude-transcript");
    into.text(conversation.as_str());
    into.text(device.as_str());
    into.text(session.as_str());
    SourceId::from_bytes(into.finish())
}

/// Every host and import for this conversation binds this destination for its
/// canonical reducer rows. Native source checkpoints stay on their own primary
/// targets; both destinations commit in the same issued source step.
pub fn canonical_timeline_source(conversation: &ObjectId) -> SourceId {
    let mut into = Encoder::new("source");
    into.text("canonical-timeline");
    into.text(conversation.as_str());
    SourceId::from_bytes(into.finish())
}
