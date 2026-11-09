//! A conversation this World hosts, in the foundation's identity: its birth
//! coordinates under the alone hub name the canonical conversation, whose scope
//! and timeline object derive from it. A World with any hub membership keeps
//! its conversations' identity at the hub, so it hosts none this way.
use fastctx_sync::{
    identity::{ObjectId, ObjectKey, ScopeKey},
    store::Error,
};
use fastctx_types::{
    ResidentId,
    sync::conversation_identity::{ConversationKey, ConversationObject, conversation_scope},
};

use super::Foundation;
use crate::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hosted {
    /// The canonical conversation; the producer's merge requests name it.
    pub conversation: ObjectId,
    /// Its conversation.timeline object, which ingestion and reads address.
    pub timeline: ObjectKey,
}

impl Foundation {
    /// The identity of `resident` when this World hosts it under its alone hub.
    pub(crate) fn hosted(&self, node: &Node, resident: &ResidentId) -> Option<Hosted> {
        if node.connected_anywhere() {
            return None;
        }
        let (about, _) = node.residents.about(resident)?;
        if !node.is_me(&about.machine) {
            return None;
        }
        let birth = ConversationKey::Birth {
            hub: self.alone.clone(),
            device: self.device.clone(),
            resident: about.id,
            born_at: about.born_at,
        };
        let conversation = birth.id();
        let scope = ScopeKey { hub: self.alone.clone(), scope: conversation_scope(&conversation) };
        let timeline = ObjectKey { scope, object: ConversationObject::Timeline.id(&conversation) };
        Some(Hosted { conversation, timeline })
    }

    /// Opens the hosted conversation's scope before its first ingestion; a
    /// conversation deleted since is refused rather than reopened.
    #[allow(dead_code, reason = "the conversation ingestion adapter's entry")]
    pub(crate) async fn open_scope(&self, hosted: &Hosted) -> Result<(), Error> {
        self.scopes.open_conversation(hosted.conversation.clone()).await.map(drop)
    }
}
