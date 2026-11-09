//! Private usage links contain no financial fields. Missing canonical birth
//! evidence leaves the original local; a ResidentId alone cannot name a scope.
use crate::{
    Error,
    source::{Binding, denied},
    usage::Usage,
};
use fastctx_sync::{
    identity::{Digest, ObjectKey},
    store::ingest::SourceTarget,
};
use fastctx_types::sync::scope::{ScopeDescriptor, ScopeSubject};

/// Resolved from the original call and the common conversation identity module.
/// The original source/login producer must already be bound by `binding`.
pub struct Evidence<'a> {
    pub binding: &'a Binding,
    pub call: &'a Digest,
    pub conversation: &'a ObjectKey,
    pub turn: Option<&'a str>,
}
pub fn authorize_source(
    value: &Usage,
    target: &SourceTarget,
    evidence: Evidence<'_>,
    scope: &ScopeDescriptor,
) -> Result<(), Error> {
    evidence.binding.check(target, "account.usage")?;
    if !matches!(evidence.binding.producer, fastctx_sync::identity::Producer::Device { .. }) {
        return Err(denied("private usage link requires its original login device"));
    }
    let Usage::ConversationLink { call, conversation, turn } = value else {
        return Err(denied("not a private usage link"));
    };
    if call != evidence.call
        || conversation != evidence.conversation
        || turn.as_deref() != evidence.turn
        || conversation.scope != evidence.binding.object.scope
        || scope.scope != conversation.scope
        || scope.acl_revision != evidence.binding.acl_revision
        || !matches!(&scope.subject, ScopeSubject::Conversation { conversation: id, .. } if id == &conversation.object)
    {
        return Err(denied("private usage link lacks its canonical conversation and original call evidence"));
    }
    Ok(())
}
