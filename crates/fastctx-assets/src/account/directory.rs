//! Existing scope facts retain their exact public wire shape. Control facts are
//! installed only after the foundation publishes the shared directory family.
use crate::{Error, json};
use fastctx_types::sync::{
    catalog::Lifecycle,
    scope::{ScopeDescriptor, ScopeOwner, ScopeSubject, ScopeVisibility},
};

/// Checks shape only. A source proof, its producer and the directory transition
/// still require the foundation's verifier and SourceAuthorizer.
pub fn decode(body: &[u8]) -> Result<ScopeDescriptor, Error> {
    let descriptor: ScopeDescriptor = json::typed(serde_json::Value::Object(json::object(body)?))?;
    if descriptor.format != 1
        || descriptor.acl_revision.0 == 0
        || descriptor.revision.revision.0 == 0
        || descriptor.lifecycle == Lifecycle::Removed
    {
        return Err(json::invalid("invalid scope identity or revision"));
    }
    if matches!(descriptor.subject, ScopeSubject::Administrators) != matches!(descriptor.owner, ScopeOwner::Hub) {
        return Err(json::invalid("only administrator scopes are hub-owned"));
    }
    if let ScopeVisibility::SinceGrant { opened_acl } = descriptor.visibility
        && (!matches!(descriptor.subject, ScopeSubject::Machine { .. })
            || opened_acl.0 == 0
            || opened_acl > descriptor.acl_revision)
    {
        return Err(json::invalid("invalid machine grant-segment fence"));
    }
    if let ScopeSubject::Conversation { shared_on, born_seal, .. } = &descriptor.subject
        && (shared_on.is_some() != born_seal.is_some() || born_seal.as_ref().is_some_and(|seal| seal.len() > 4096))
    {
        return Err(json::invalid("shared origin requires a bounded birth seal"));
    }
    Ok(descriptor)
}

/// A scoped public fact, not a complete legacy View or a permission grant.
pub fn project_scope(body: &[u8]) -> Result<ScopeDescriptor, Error> {
    decode(body)
}
