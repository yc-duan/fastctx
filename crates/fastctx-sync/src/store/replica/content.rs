//! Download requests are issued only for roots reached from verified evidence.
use super::{SqliteReplica, rows};
use crate::{
    identity::{Digest, ObjectKey},
    manifest::{ContentRef, ManifestReply, Representation},
    store::{Error, FutureResult, access::Custody, replica::ReplicaContext, sql},
};

/// A local keyset position, not a network capability.
#[derive(Clone, Debug)]
pub struct ContentPosition {
    pub digest: Digest,
    pub manifest: bool,
}
/// The receiver alone creates this handle; publication checks its durable
/// declaration and current holding again. It grants no body read permission.
#[derive(Clone)]
pub struct ContentRequest {
    pub(crate) custody: Custody,
    pub(crate) root: ContentRef,
}
impl ContentRequest {
    pub fn root(&self) -> &ContentRef {
        &self.root
    }
    pub fn object(&self) -> &ObjectKey {
        &self.custody.object
    }
    pub fn kind(&self) -> &str {
        self.custody.kind.registration().id
    }
    pub fn holding(&self) -> &crate::cursor::HoldingPeriod {
        &self.custody.holding
    }
}
pub struct MissingContent {
    pub requests: Vec<ContentRequest>,
    pub next: Option<ContentPosition>,
    /// Known missing nodes or unexpanded local pages remain, even if this page
    /// contains no network requests. Continue at next, or start a new bounded pass.
    pub pending: bool,
}
impl SqliteReplica {
    /// Examines at most `max` outstanding nodes. Existing pages extend the
    /// authenticated closure in short transactions; completed nodes leave the index.
    pub fn missing_content(
        &self,
        context: ReplicaContext,
        object: ObjectKey,
        kind: String,
        after: Option<ContentPosition>,
        max: u32,
    ) -> FutureResult<'_, MissingContent> {
        Box::pin(async move {
            self.connection(&context)?;
            let custody =
                rows::custody(&context, &object, &kind).map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
            self.blocks.replica_missing(custody, after, max).await
        })
    }
    pub fn receive_block(
        &self,
        request: ContentRequest,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, ()> {
        Box::pin(async move {
            if request.root != ContentRef::from(representation.content.clone()) {
                return Err(Error::Invalid("downloaded block differs from the issued content request".into()));
            }
            self.blocks.replica_block(request, representation, encoded).await
        })
    }
    pub fn receive_manifest(&self, request: ContentRequest, page: ManifestReply) -> FutureResult<'_, ()> {
        Box::pin(async move {
            if request.root != (ContentRef::Manifest { root: page.root.clone() }) {
                return Err(Error::Invalid("downloaded manifest differs from the issued content request".into()));
            }
            self.blocks.replica_manifest(request, page).await
        })
    }
}
