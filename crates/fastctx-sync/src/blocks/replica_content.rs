//! A received proof authorizes fetching its closure, not claiming arbitrary local
//! hashes. Each publication rechecks the issued request at both SQL boundaries.
use super::SqliteBlocks;
use crate::{
    blocks::{access, codec, manifest, pages, pins, replica_commit, rows, uploads},
    identity::{Digest, OperationId},
    manifest::{BlobId, ContentRef, DigestAlgorithm, ManifestPage, ManifestReply, ManifestRoot, Representation},
    store::{
        Error,
        access::Custody,
        boundary::{self, Role},
        replica::{ContentPosition, ContentRequest, MissingContent},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};

impl SqliteBlocks {
    pub(crate) async fn replica_missing(
        &self,
        custody: Custody,
        after: Option<ContentPosition>,
        max: u32,
    ) -> Result<MissingContent, Error> {
        if max == 0 || max > 64 {
            return Err(Error::Invalid("missing content pages contain 1 to 64 nodes".into()));
        }
        let after_key = after
            .as_ref()
            .map(|a| (a.digest.as_str(), if a.manifest { "manifest" } else { "block" }))
            .unwrap_or(("", ""));
        let nodes=self.db.read_now(|db| {
            pins::authorize(db,&self.holder,self.policy.as_ref(),&custody)?;
            let scope=&custody.object.scope;
            let mut q=db.prepare_cached("SELECT hash,root_kind,decoded_bytes FROM sync_replica_content INDEXED BY sync_replica_content_pending WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND done=0 AND (hash,root_kind)>(?5,?6) ORDER BY hash,root_kind LIMIT ?7")?;
            let found=q.query_map(params![scope.hub.to_string(),scope.scope.as_str(),custody.object.object.as_str(),custody.kind.registration().id,after_key.0,after_key.1,max+1],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<u32>>(2)?)))?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
            found.into_iter().map(|(hash,form,len)| {
                let id=BlobId{domain:scope.clone(),algorithm:DigestAlgorithm::Sha256,digest:Digest::try_from(hash).map_err(sql::invalid)?};
                match form.as_str() {"block"=>Ok(ContentRef::Single{block:id,decoded_bytes:len.ok_or_else(||sql::invalid("block request lacks its length"))?}),"manifest"=>Ok(ContentRef::Manifest{root:ManifestRoot(id)}),_=>Err(sql::invalid("unknown replica reference form"))}
            }).collect::<SqlResult<Vec<_>>>()
        }).map_err(|e|sql::error(e,self.worker.clone()))?;
        let next = (nodes.len() > max as usize).then(|| {
            let root = &nodes[max as usize - 1];
            ContentPosition { digest: root.id().digest.clone(), manifest: matches!(root, ContentRef::Manifest { .. }) }
        });
        let mut requests = Vec::new();
        for root in nodes.into_iter().take(max as usize) {
            let request = ContentRequest { custody: custody.clone(), root };
            if !self.inspect_replica(&request).await? {
                requests.push(request);
            }
        }
        let pending=self.db.read_now(|db| {
            pins::authorize(db,&self.holder,self.policy.as_ref(),&custody)?;
            let scope=&custody.object.scope;
            db.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_content INDEXED BY sync_replica_content_pending WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND done=0)",params![scope.hub.to_string(),scope.scope.as_str(),custody.object.object.as_str(),custody.kind.registration().id],|r|r.get(0)).map_err(Into::into)
        }).map_err(|e|sql::error(e,self.worker.clone()))?;
        Ok(MissingContent { requests, next, pending })
    }
    async fn inspect_replica(&self, request: &ContentRequest) -> Result<bool, Error> {
        self.db.read_now(|db| require(db, self, request)).map_err(|e| sql::error(e, self.worker.clone()))?;
        match &request.root {
            ContentRef::Single { .. } => {
                let complete = self
                    .db
                    .read_now(|db| rows::complete(db, &request.root))
                    .map_err(|e| sql::error(e, self.worker.clone()))?;
                if complete {
                    self.finish_replica(request.clone(), true).await?;
                }
                Ok(complete)
            }
            ContentRef::Manifest { root } => {
                let Some(page) = self.stored_page(root)? else { return Ok(false) };
                // Publication may have committed just before the receiver died.
                self.finish_replica(request.clone(), false).await?;
                let checked = manifest::validate(&request.custody.object.scope, page)?;
                let page: ManifestPage =
                    serde_json::from_str(&checked.reply.page_json).map_err(|e| Error::Corrupt(e.to_string()))?;
                if let ManifestPage::Metadata { codec, payload, references } = page {
                    let payload_root = ContentRef::from(payload.clone());
                    let custody = request.custody.clone();
                    let parent = request.clone();
                    let blocks = self.clone();
                    self.db
                        .write(8192, move |db| {
                            let _role = boundary::enter_typed_write(Role::Replica);
                            require(db, &blocks, &parent)?;
                            // The exact manifest bytes bind this payload even before
                            // its schema has been checked. Other references wait.
                            replica_commit::adopt(db, &custody, &payload_root)
                        })
                        .await
                        .map_err(|e| sql::error(e, self.worker.clone()))?;
                    let Some((bytes, _permit)) = self.stored_block(&payload)? else { return Ok(true) };
                    crate::blocks::metadata::validate(&root.0.domain, codec, &payload, &bytes, &references)?;
                }
                let children = checked
                    .references
                    .into_iter()
                    .map(|r| match r {
                        manifest::Reference::Block { id, decoded_bytes } => {
                            ContentRef::Single { block: id, decoded_bytes }
                        }
                        manifest::Reference::Manifest { root } => ContentRef::Manifest { root },
                    })
                    .collect::<Vec<_>>();
                let request = request.clone();
                let blocks = self.clone();
                self.db
                    .write(checked.reply.page_json.len() * 4 + 16384, move |db| {
                        let _role = boundary::enter_typed_write(Role::Replica);
                        require(db, &blocks, &request)?;
                        let id = request.root.id();
                        db.execute(
                            "UPDATE sync_asset_manifests SET validated=1 WHERE hub=?1 AND scope=?2 AND hash=?3",
                            params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
                        )?;
                        for root in &children {
                            replica_commit::adopt(db, &request.custody, root)?;
                        }
                        mark_done(db, &request)?;
                        pages::refresh(db, &id.domain, &id.digest)?;
                        crate::scheduler::content_changed(
                            db,
                            &request.custody.holding,
                            format!("complete:{}", id.digest.as_str()),
                        )?;
                        crate::scheduler::content_changed(
                            db,
                            &request.custody.holding,
                            format!("access:{}", id.digest.as_str()),
                        )?;
                        wake(db, &request.custody)
                    })
                    .await
                    .map_err(|e| sql::error(e, self.worker.clone()))?;
                Ok(true)
            }
        }
    }
    pub(crate) async fn replica_block(
        &self,
        request: ContentRequest,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> Result<(), Error> {
        // Validation also applies to duplicate delivery; no unverified bytes are
        // accepted merely because another representation already exists.
        {
            let _budget =
                self.budget.acquire(encoded.capacity() + codec::decode_workspace(representation.envelope.encoding))?;
            codec::validate(&representation, &encoded)?;
        }
        self.db.read_now(|db| require(db, self, &request)).map_err(|e| sql::error(e, self.worker.clone()))?;
        let present =
            self.db.read_now(|db| rows::complete(db, &request.root)).map_err(|e| sql::error(e, self.worker.clone()))?;
        if present {
            return self.finish_replica(request, true).await;
        }
        let operation = operation(&request)?;
        self.publish_block(request.custody.clone(), operation, None, representation, encoded).await?;
        self.finish_replica(request, true).await
    }
    pub(crate) async fn replica_manifest(&self, request: ContentRequest, page: ManifestReply) -> Result<(), Error> {
        manifest::validate(&request.custody.object.scope, page.clone())?;
        self.db.read_now(|db| require(db, self, &request)).map_err(|e| sql::error(e, self.worker.clone()))?;
        let ContentRef::Manifest { root } = &request.root else {
            return Err(Error::Invalid("manifest request is not a manifest".into()));
        };
        if self.stored_page(root)?.is_none() {
            let operation = operation(&request)?;
            self.publish_manifest(request.custody.clone(), operation, None, page).await?;
        }
        self.finish_replica(request.clone(), false).await?;
        self.inspect_replica(&request).await?;
        Ok(())
    }
    async fn finish_replica(&self, request: ContentRequest, done: bool) -> Result<(), Error> {
        let blocks = self.clone();
        let operation = operation(&request)?;
        self.db.write(16384,move |db| {
            let _role=boundary::enter_typed_write(Role::Replica);
            require(db,&blocks,&request)?;
            let id=request.root.id();
            let generation=db.query_row("SELECT generation FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND hash=?5 AND root_kind=?6 AND phase!='released'",params![id.domain.hub.to_string(),id.domain.scope.as_str(),operation.as_str(),request.custody.object.object.as_str(),id.digest.as_str(),pins::shape(&request.root).0],|r|crate::store::sql::counter(r,0)).optional()?;
            if let Some(generation)=generation {
                uploads::close(db,&blocks.holder,&request.custody,&operation)?;
                pins::release(db,&request.custody,&crate::manifest::Pin{operation,object:request.custody.object.clone(),kind:request.custody.kind.registration().id.into(),root:request.root.clone(),generation:crate::cursor::Revision(generation),holding:request.custody.holding.clone()})?;
            }
            let changed=done && mark_done(db,&request)?;
            if changed || generation.is_some() {wake(db,&request.custody)?;}
            Ok(())
        }).await.map_err(|e|sql::error(e,self.worker.clone()))
    }
}
fn require(db: &Connection, blocks: &SqliteBlocks, request: &ContentRequest) -> SqlResult<()> {
    pins::authorize(db, &blocks.holder, blocks.policy.as_ref(), &request.custody)?;
    let custody = &request.custody;
    let scope = &custody.object.scope;
    let (form, length) = pins::shape(&request.root);
    let found:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_content WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind=?6 AND decoded_bytes IS ?7)",params![scope.hub.to_string(),scope.scope.as_str(),custody.object.object.as_str(),custody.kind.registration().id,request.root.id().digest.as_str(),form,length],|r|r.get(0))?;
    if !found {
        return Err(sql::invalid("content was not declared by a verified replica root"));
    }
    // Grants are checked independently: retirement cannot be undone by a stale
    // download queue row, and the queue never creates authority on its own.
    if !access::has_grant(db, custody, &request.root)? {
        return Err(sql::invalid("replica content declaration has been retired"));
    }
    Ok(())
}
fn mark_done(db: &Connection, request: &ContentRequest) -> SqlResult<bool> {
    let c = &request.custody;
    let id = request.root.id();
    let changed=db.execute("UPDATE sync_replica_content SET done=1 WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind=?6 AND done=0",params![id.domain.hub.to_string(),id.domain.scope.as_str(),c.object.object.as_str(),c.kind.registration().id,id.digest.as_str(),pins::shape(&request.root).0])?;
    Ok(changed != 0)
}
fn wake(db: &Connection, custody: &Custody) -> SqlResult<()> {
    if custody.kind.registration().id == "conversation.timeline" {
        let local = crate::store::local_head::advance(db)?;
        crate::scheduler::projection_changed(db, "derived.timeline", &custody.object, local)?;
    }
    Ok(())
}
fn operation(request: &ContentRequest) -> Result<OperationId, Error> {
    let bytes = serde_json::to_vec(&(
        &request.custody.object,
        request.custody.kind.registration().id,
        &request.custody.holding,
        &request.root,
    ))
    .map_err(|e| Error::Invalid(e.to_string()))?;
    let mut material = b"fastctx-sync/replica-content/v1\0".to_vec();
    material.extend_from_slice(&bytes);
    Ok(OperationId::from_bytes(Digest::of(&material).bytes()))
}
