//! A conversation's scope follows its ownership record. When a host registers
//! a conversation with its birth facts, or registers it again, the hub signs or
//! confirms the description of that conversation's scope.
use fastctx_sync::{
    catalog::Lifecycle,
    cursor::Revision,
    identity::{AccountId, Digest, OperationId, ScopeKey},
    scope::{AccountIdentity, ScopeSubject, ScopeVisibility},
    store::{Error, directory::ScopeDescription},
};
use fastctx_types::{owned_records::Conversation, sync::conversation_identity::conversation_scope};
use jiff::Timestamp;

use super::Foundation;

impl Foundation {
    /// `opened` is the owning account's opening time in the hub's directory. A
    /// conversation registered without birth facts has no description yet.
    pub(crate) async fn describe_conversation(
        &self,
        opened: Timestamp,
        conversation: &Conversation,
    ) -> Result<(), Error> {
        let Some(identity) = conversation.identity.as_deref().cloned() else { return Ok(()) };
        let directory = &self.authority()?.directory;
        let owner = AccountIdentity {
            id: AccountId::from_legacy(&self.hub, &conversation.account, opened),
            name: conversation.account.clone(),
            opened,
        };
        let scope = ScopeKey { hub: self.hub.clone(), scope: conversation_scope(&identity) };
        let acl = directory.current(scope.clone()).await?.map_or(Revision(1), |current| current.acl_revision);
        let description = ScopeDescription {
            scope: scope.clone(),
            owner,
            subject: ScopeSubject::Conversation {
                conversation: identity,
                shared_on: conversation.shared_on.clone(),
                born_seal: conversation.born_seal.clone(),
            },
            visibility: ScopeVisibility::Current,
            acl_revision: acl,
            lifecycle: Lifecycle::Live,
        };
        let seed = format!("fastctx-hub/conversation-scope/v1\0{}\0{}", scope.scope.as_str(), acl.0);
        directory.publish(description, OperationId::from_bytes(Digest::of(seed.as_bytes()).bytes())).await.map(drop)
    }
}
