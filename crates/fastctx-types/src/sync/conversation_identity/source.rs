//! A conversation's source streams belong to their registered canonical objects.
use super::ConversationObject;
use crate::sync::identity::ObjectId;

/// Returns the object identity defined for this conversation source kind.
/// A kind without a canonical object contract is unavailable to source transfer.
pub fn source_object(conversation: &ObjectId, kind: &str) -> Option<ObjectId> {
    let object = match kind {
        "conversation.timeline" => ConversationObject::Timeline,
        "conversation.agents" => ConversationObject::Agents,
        _ => return None,
    };
    Some(object.id(conversation))
}
