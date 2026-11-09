use crate::{
    conversation::Dependency,
    identity::{Digest, ObjectId},
};
use serde::Serialize;

pub(super) fn namespace(conversation: &ObjectId) -> Vec<u8> {
    format!("canon/v1/{}/", conversation.as_str()).into_bytes()
}
pub(super) fn hash(value: &impl Serialize) -> String {
    // All callers pass typed fields whose serialization is infallible.
    Digest::of(&serde_json::to_vec(value).expect("typed index key serializes")).as_str().to_owned()
}
pub(super) fn item(id: &ObjectId) -> String {
    format!("item/{}", id.as_str())
}
pub(super) fn dependency(dep: &Dependency) -> String {
    match dep {
        Dependency::Item(id) => format!("item/{}", id.as_str()),
        Dependency::Placement(id) => format!("placement/{}", id.as_str()),
        Dependency::Turn(id) => format!("turn/{}", id.as_str()),
        Dependency::Parent(id) => format!("parent/{}", id.as_str()),
        Dependency::Append { item, previous } => format!("append/{}/{}", item.as_str(), previous.as_str()),
        Dependency::Generation { source, previous } => format!("generation/{}/{}", source.as_str(), previous.as_str()),
    }
}
