//! Replica supply uses the feed read gate, then the hub's adopted object roots.
mod membership;
mod ranges;
use super::{Peer, Service, read};
use crate::{
    blocks::SqliteBlocks,
    cursor::{Holder, HoldingPeriod},
    identity::{ObjectKey, ScopeKey},
    manifest::{BlobId, BlockStore, ContentRef, Descriptor, ManifestRoot},
    registry::{REGISTRY, RegisteredKind},
    replicate::{BlockRequest, Received},
    store::{
        Error,
        access::{AccessContext, Custody, HoldingUse, Principal},
        scopes,
    },
    transfer::{KindVersion, Message},
};
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};

impl Service {
    fn supply_blocks(&self) -> Result<&SqliteBlocks, Error> {
        self.sources
            .as_ref()
            .map(|authority| authority.blocks())
            .ok_or_else(|| Error::Unavailable("this hub has no content authority installed".into()))
    }
    /// Rechecks the same remote-device gate used by feed and stream reads.
    pub async fn recheck(&self, peer: &Peer, holding: &HoldingPeriod) -> Result<(), Error> {
        self.recheck_scope(peer, &holding.scope, Some(holding)).await
    }
    async fn recheck_scope(&self, peer: &Peer, scope: &ScopeKey, holding: Option<&HoldingPeriod>) -> Result<(), Error> {
        if scope.hub != *self.directory.hub() {
            return Err(Error::Denied("holding belongs to another hub".into()));
        }
        let (peer, scope, holding, policy) = (peer.clone(), scope.clone(), holding.cloned(), self.policy.clone());
        self.directory
            .database()
            .read(move |db| {
                Ok((|| -> read::Result<()> {
                    let snapshot = db.unchecked_transaction()?;
                    read::authorize(&snapshot, &peer, &scope, holding.as_ref(), policy.as_ref())?;
                    snapshot.commit()?;
                    Ok(())
                })())
            })
            .await
            .map_err(|error| read::ReadError::from(error).0)?
            .map_err(|error| error.0)
    }
    /// Called before every metadata response fragment; committed write receipts
    /// are delivered without treating them as a new read of the written object.
    pub async fn recheck_message(&self, peer: &Peer, request: &Message) -> Result<(), Error> {
        match request {
            Message::GetManifest { holding, root } | Message::WantBlocks { holding, manifest: root, .. } => {
                self.custody(peer, holding, &ContentRef::Manifest { root: root.clone() }, None).await?;
            }
            Message::GetDescriptor { holding, blob } => {
                self.recheck(peer, holding).await?;
                let root = self.block_reference(&holding.scope, blob).await?;
                self.custody(peer, holding, &root, None).await?;
            }
            Message::Changes { holding, .. } | Message::StreamRange { holding, .. } => {
                self.recheck(peer, holding).await?
            }
            Message::GetEpoch { scope, .. } => self.recheck_scope(peer, scope, None).await?,
            Message::ProbeSource { upload, .. } | Message::PublishSourceManifest { upload, .. } => {
                self.check_source_transfer(peer, upload).await?;
            }
            _ => {}
        }
        Ok(())
    }
    async fn custody(
        &self,
        peer: &Peer,
        holding: &HoldingPeriod,
        root: &ContentRef,
        expected: Option<(ObjectKey, KindVersion)>,
    ) -> Result<Custody, Error> {
        self.supply_blocks()?;
        root.validate().map_err(Error::Invalid)?;
        if root.id().domain != holding.scope || holding.scope.hub != *self.directory.hub() {
            return Err(Error::Denied("content is outside the requested scope".into()));
        }
        let (peer, holding, root, policy) = (peer.clone(), holding.clone(), root.clone(), self.policy.clone());
        self.directory
            .database()
            .read(move |db| {
                Ok((|| -> read::Result<Custody> {
                    let snapshot = db.unchecked_transaction()?;
                    let descriptor =
                        read::authorize(&snapshot, &peer, &holding.scope, Some(&holding), policy.as_ref())?;
                    let (object, kind) = grant(&snapshot, &holding.scope, &root, expected.as_ref(), &descriptor)?;
                    let state = scopes::current(&snapshot, &holding.scope)?.ok_or(Error::Revoked)?;
                    let custody = Custody {
                        context: AccessContext {
                            principal: Principal::LocalService,
                            scope: holding.scope.clone(),
                            observed_acl: state.acl_revision,
                        },
                        object,
                        kind,
                        purpose: HoldingUse::Replica,
                        holding: HoldingPeriod {
                            holder: Holder::Hub { hub: holding.scope.hub.clone() },
                            scope: holding.scope.clone(),
                            acl_revision: state.acl_revision,
                            publication_epoch: state.publication_epoch,
                        },
                    };
                    snapshot.commit()?;
                    Ok(custody)
                })())
            })
            .await
            .map_err(|error| read::ReadError::from(error).0)?
            .map_err(|error| error.0)
    }
    /// Revalidates the adopted root as well as the receiving device's tenure.
    pub async fn check_supply(&self, peer: &Peer, request: &BlockRequest) -> Result<(), Error> {
        self.recheck(peer, &request.holding).await?;
        self.custody(peer, &request.holding, &request.root, Some((request.object.clone(), request.kind.clone())))
            .await?;
        Ok(())
    }
    /// Returns a verified decoded block with its working-set admission retained.
    /// Root membership and adoption never follow a caller-authored staging pin.
    pub async fn supply(&self, peer: &Peer, request: BlockRequest) -> Result<Received<Vec<u8>>, Error> {
        if request.block.id.domain != request.object.scope
            || request.block.decoded_bytes > fastctx_types::sync::BLOCK_BYTES
        {
            return Err(Error::Denied("block is outside the requested object".into()));
        }
        let custody = self.custody(peer, &request.holding, &request.root, Some((request.object, request.kind))).await?;
        let blocks = self.supply_blocks()?;
        self.member(&custody, &request.root, &ContentRef::from(request.block.clone())).await?;
        // Accepted roots and checked closure edges authorize reads before the
        // asynchronous descendant grant cache has caught up.
        blocks.require_adopted(&custody, &request.root)?;
        let (decoded, permit) = blocks
            .stored_block(&request.block)?
            .ok_or_else(|| Error::Unavailable("the granted block is not stored".into()))?;
        self.custody(
            peer,
            &request.holding,
            &request.root,
            Some((
                custody.object,
                KindVersion { kind: custody.kind.registration().id.into(), schema: custody.kind.registration().schema },
            )),
        )
        .await?;
        Ok(Received::new(decoded, permit))
    }
    pub(super) async fn manifest(
        &self,
        peer: &Peer,
        holding: HoldingPeriod,
        root: ManifestRoot,
    ) -> Result<Message, Error> {
        let reference = ContentRef::Manifest { root: root.clone() };
        self.recheck(peer, &holding).await?;
        let custody = self.custody(peer, &holding, &reference, None).await?;
        let reply = self.supply_blocks()?.manifest(custody, root).await?;
        self.custody(peer, &holding, &reference, None).await?;
        Ok(Message::Manifest { holding, reply })
    }
    pub(super) async fn descriptor(&self, peer: &Peer, holding: HoldingPeriod, blob: BlobId) -> Result<Message, Error> {
        if blob.domain != holding.scope {
            return Err(Error::Denied("descriptor is outside the requested scope".into()));
        }
        self.supply_blocks()?;
        self.recheck(peer, &holding).await?;
        let root = self.block_reference(&holding.scope, &blob).await?;
        let custody = self.custody(peer, &holding, &root, None).await?;
        let descriptor: Descriptor = self.supply_blocks()?.descriptor(custody, blob).await?;
        self.custody(peer, &holding, &root, None).await?;
        Ok(Message::Descriptor { holding, descriptor })
    }
    async fn block_reference(&self, scope: &ScopeKey, id: &BlobId) -> Result<ContentRef, Error> {
        if &id.domain != scope {
            return Err(Error::Denied("descriptor is outside the requested scope".into()));
        }
        let id = id.clone();
        self.directory
            .database()
            .read(move |db| {
                let length = db
                    .query_row(
                        "SELECT decoded_bytes FROM sync_asset_access
                 WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind='block' LIMIT 1",
                        params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
                        |row| row.get::<_, u32>(0),
                    )
                    .optional()?;
                Ok(length.map(|decoded_bytes| ContentRef::Single { block: id, decoded_bytes }))
            })
            .await
            .map_err(|error| read::ReadError::from(error).0)?
            .ok_or_else(|| Error::Unavailable("metadata is not yet granted to a live accepted object".into()))
    }
}
fn kind(name: &str) -> read::Result<RegisteredKind> {
    REGISTRY
        .asset(name)
        .map(|kind| kind.registered())
        .or_else(|_| REGISTRY.projection(name).map(|kind| kind.registered()))
        .map_err(|_| read::unavailable("the adopted content kind is not installed"))
}
fn grant(
    db: &Connection,
    scope: &ScopeKey,
    root: &ContentRef,
    expected: Option<&(ObjectKey, KindVersion)>,
    descriptor: &crate::scope::ScopeDescriptor,
) -> read::Result<(ObjectKey, RegisteredKind)> {
    let (form, length) = match root {
        ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
        ContentRef::Manifest { .. } => ("manifest", None),
    };
    let (object, wanted_kind) =
        expected.map(|(object, kind)| (Some(object.object.as_str()), Some(kind.kind.as_str()))).unwrap_or((None, None));
    if expected.is_some_and(|(object, _)| &object.scope != scope) {
        return Err(Error::Denied("object is outside the requested scope".into()).into());
    }
    let mut query = db.prepare_cached(
        "SELECT a.object,a.kind FROM sync_asset_access a
         WHERE a.hub=?1 AND a.scope=?2 AND a.hash=?3 AND a.root_kind=?4
           AND a.decoded_bytes IS ?5 AND (?6 IS NULL OR a.object=?6)
           AND (?7 IS NULL OR a.kind=?7)
           AND NOT EXISTS(SELECT 1 FROM sync_tombstones t
                          WHERE t.hub=a.hub AND t.scope=a.scope AND t.object=a.object)
         ORDER BY a.object,a.kind",
    )?;
    let mut rows = query.query(params![
        scope.hub.to_string(),
        scope.scope.as_str(),
        root.id().digest.as_str(),
        form,
        length,
        object,
        wanted_kind
    ])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        let Ok(registered) = kind(&name) else { continue };
        let object: crate::identity::ObjectId = row.get::<_, String>(0)?.try_into().map_err(read::corrupt)?;
        if let crate::scope::ScopeSubject::Conversation { conversation, .. } = &descriptor.subject
            && let Some(canonical) = fastctx_types::sync::conversation_identity::source_object(conversation, &name)
            && object != canonical
        {
            continue;
        }
        if expected.is_some_and(|(_, wanted)| wanted.schema != registered.registration().schema) {
            return Err(read::unavailable("requested schema is not installed"));
        }
        return Ok((ObjectKey { scope: scope.clone(), object }, registered));
    }
    if expected.is_some() {
        Err(Error::Denied("root is not granted to a live accepted object".into()).into())
    } else {
        Err(read::unavailable("metadata is not yet granted to a live accepted object"))
    }
}
