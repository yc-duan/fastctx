//! Two-phase content publication: durable pin, file IO, then a FULL index commit.
use super::{access, budget::Budget, codec, files::Files, leases::Leases, manifest, pages, pins, rows};
use crate::{
    cursor::Holder,
    identity::{Digest, OperationId, ScopeKey},
    manifest::{
        BlobId, BlockStore, ContentRef, Descriptor, ManifestReply, ManifestRoot, Pin, PublishedBlock, Representation,
    },
    scheduler,
    store::{
        Error, FutureResult,
        access::{AccessPolicy, Custody},
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::Database;
use std::{path::PathBuf, sync::Arc};

#[path = "cleanup.rs"]
mod cleanup;
#[path = "gc.rs"]
mod gc;
#[path = "metadata_custody.rs"]
mod metadata_custody;
#[path = "metadata_work.rs"]
mod metadata_work;
#[path = "root_store.rs"]
mod root_store;
#[path = "work.rs"]
mod work;

#[derive(Clone)]
pub struct SqliteBlocks {
    db: Database,
    holder: Holder,
    policy: Arc<dyn AccessPolicy>,
    files: Arc<Files>,
    leases: Arc<Leases>,
    budget: Arc<Budget>,
    worker: OperationId,
}
impl SqliteBlocks {
    /// Service composition supplies an exclusive content root and its machine's
    /// buffer budget. All clones share file ownership, IO leases and admission.
    pub fn new(
        db: Database,
        root: PathBuf,
        holder: Holder,
        policy: Arc<dyn AccessPolicy>,
        worker: OperationId,
        buffer_bytes: usize,
    ) -> Result<Self, Error> {
        let budget = Budget::new(buffer_bytes)?;
        Ok(Self {
            db,
            holder,
            policy,
            files: Arc::new(Files::open(root)?),
            leases: Arc::new(Leases::default()),
            budget,
            worker,
        })
    }
    /// The service calls this after committing the scope's invalidation. Cleanup
    /// may proceed only when this returns true; repeated calls are idempotent.
    pub fn close(&self, scope: &ScopeKey) -> Result<bool, Error> {
        self.db
            .read_now(|db| {
                let enabled = db.query_row(
                    "SELECT enabled FROM sync_scopes WHERE hub=?1 AND scope=?2",
                    fastctx_storage::rusqlite::params![scope.hub.to_string(), scope.scope.as_str()],
                    |row| row.get::<_, bool>(0),
                )?;
                if enabled {
                    return Err(sql::invalid("scope must be durably disabled before draining content IO"));
                }
                Ok(())
            })
            .map_err(|error| sql::error(error, self.worker.clone()))?;
        Ok(self.leases.close(scope))
    }
    pub fn reopen(&self, scope: &ScopeKey) -> Result<(), Error> {
        self.leases.reopen(scope)
    }
    /// Performs one bounded unit of a leased manifest task. The caller persists
    /// the returned outcome with the common Scheduler before claiming more work.
    pub fn process(&self, lease: crate::scheduler::Lease) -> FutureResult<'_, crate::scheduler::Outcome> {
        Box::pin(work::process(self, lease))
    }
    fn same_scope(custody: &Custody, id: &BlobId) -> Result<(), Error> {
        if custody.object.scope != id.domain {
            return Err(Error::Denied("content belongs to another object scope".into()));
        }
        Ok(())
    }
}
impl BlockStore for SqliteBlocks {
    fn put(
        &self,
        custody: Custody,
        operation: OperationId,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'_, PublishedBlock> {
        Box::pin(async move {
            Self::same_scope(&custody, &representation.content.id)?;
            representation.validate().map_err(Error::Invalid)?;
            if encoded.len() != representation.encoded_bytes as usize {
                return Err(Error::Invalid("encoded block length differs from its descriptor".into()));
            }
            let _buffer =
                self.budget.acquire(encoded.capacity() + codec::decode_workspace(representation.envelope.encoding))?;
            codec::validate(&representation, &encoded)?;
            let _lease = self.leases.write(&representation.content.id)?;
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let request = custody.clone();
            let id = operation.clone();
            let (stored, pin) = self
                .db
                .write(encoded.len() + 16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Content);
                    pins::authorize(db, &holder, policy.as_ref(), &request)?;
                    let pin = pins::stage(db, &request, id.clone(), ContentRef::from(representation.content.clone()))?;
                    let stored = rows::reserve(db, representation, id)?;
                    if stored.verified {
                        pins::ready(db, &request, &pin)?;
                        // New object custody can unblock an existing index payload.
                        scheduler::content_changed(
                            db,
                            &request.holding,
                            format!("complete:{}", stored.representation.content.id.digest.as_str()),
                        )?;
                    }
                    Ok((stored, pin))
                })
                .await
                .map_err(|error| sql::error(error, operation.clone()))?;
            let content = stored.representation.content.clone();
            let representation = stored.representation.id();
            if !stored.verified {
                self.files.put(&stored.key, &encoded)?;
                let policy = self.policy.clone();
                let holder = self.holder.clone();
                let request = custody;
                let confirmed = pin.clone();
                self.db
                    .write(16384, move |db| {
                        let _boundary = boundary::enter_typed_write(Role::Content);
                        pins::authorize(db, &holder, policy.as_ref(), &request)?;
                        pins::ready(db, &request, &confirmed)?;
                        rows::publish(db, &stored)?;
                        if !rows::complete(db, &confirmed.root)? {
                            return Err(sql::invalid("published block is not complete under its pinned reference"));
                        }
                        scheduler::content_changed(
                            db,
                            &request.holding,
                            format!("complete:{}", stored.representation.content.id.digest.as_str()),
                        )
                    })
                    .await
                    .map_err(|error| sql::error(error, operation))?;
            }
            Ok(PublishedBlock { content, representation, pin })
        })
    }
    fn put_manifest(&self, custody: Custody, operation: OperationId, page: ManifestReply) -> FutureResult<'_, Pin> {
        Box::pin(async move {
            Self::same_scope(&custody, &page.root.0)?;
            let _buffer = self.budget.acquire(page.page_json.len() * 2 + 4096)?;
            let checked = manifest::validate(&custody.object.scope, page)?;
            let _lease = self.leases.write(&checked.reply.root.0)?;
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let id = operation.clone();
            self.db
                .write(checked.reply.page_json.len() * 2 + 16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Content);
                    pins::authorize(db, &holder, policy.as_ref(), &custody)?;
                    let pin = pins::stage(db, &custody, id, ContentRef::Manifest { root: checked.reply.root.clone() })?;
                    pages::put(db, &checked)?;
                    pins::ready(db, &custody, &pin)?;
                    if !checked.metadata {
                        access::expand(
                            db,
                            &custody.object,
                            custody.kind.registration().id,
                            &checked.reply.root,
                            &checked.references,
                            &custody.holding,
                        )?;
                    } else {
                        scheduler::content_changed(
                            db,
                            &custody.holding,
                            format!("metadata:{}", checked.reply.root.0.digest.as_str()),
                        )?;
                    }
                    scheduler::content_changed(
                        db,
                        &custody.holding,
                        format!("complete:{}", checked.reply.root.0.digest.as_str()),
                    )?;
                    scheduler::content_changed(
                        db,
                        &custody.holding,
                        format!("access:{}", checked.reply.root.0.digest.as_str()),
                    )?;
                    Ok(pin)
                })
                .await
                .map_err(|error| sql::error(error, operation))
        })
    }
    fn descriptor(&self, custody: Custody, id: BlobId) -> FutureResult<'_, Descriptor> {
        Box::pin(async move {
            Self::same_scope(&custody, &id)?;
            let _lease = self.leases.read(&id)?;
            let holder = self.holder.clone();
            let policy = self.policy.clone();
            self.db
                .read(move |db| {
                    pins::authorize_read(db, &holder, policy.as_ref(), &custody)?;
                    let reference = access::block_ref(db, &custody, &id)?;
                    let descriptor = rows::descriptor(db, &id)?;
                    if ContentRef::from(descriptor.content.clone()) != reference {
                        return Err(sql::invalid("content length differs from the authorized reference"));
                    }
                    Ok(descriptor)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn manifest(&self, custody: Custody, root: ManifestRoot) -> FutureResult<'_, ManifestReply> {
        Box::pin(async move {
            Self::same_scope(&custody, &root.0)?;
            let _lease = self.leases.read(&root.0)?;
            let holder = self.holder.clone();
            let policy = self.policy.clone();
            self.db
                .read(move |db| {
                    pins::authorize_read(db, &holder, policy.as_ref(), &custody)?;
                    access::require(db, &custody, &ContentRef::Manifest { root: root.clone() })?;
                    pages::get(db, &root)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    fn read(&self, custody: Custody, id: BlobId, representation: Digest) -> FutureResult<'_, Vec<u8>> {
        Box::pin(async move {
            Self::same_scope(&custody, &id)?;
            let _lease = self.leases.read(&id)?;
            let descriptor = self.descriptor(custody.clone(), id.clone()).await?;
            let selected = descriptor
                .representations
                .into_iter()
                .find(|value| value.id() == representation)
                .ok_or_else(|| Error::Unavailable("selected representation is not present".into()))?;
            let _buffer = self.budget.acquire(selected.encoded_bytes as usize)?;
            let stored = self
                .db
                .read(move |db| rows::representation(db, &selected))
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))?;
            let bytes = self.files.read(&stored.key, stored.representation.encoded_bytes)?;
            if Digest::of(&bytes) != stored.representation.encoded_digest {
                return Err(Error::Corrupt("published content failed its encoded digest".into()));
            }
            let holder = self.holder.clone();
            let policy = self.policy.clone();
            self.db
                .read(move |db| {
                    pins::authorize_read(db, &holder, policy.as_ref(), &custody)?;
                    access::block_ref(db, &custody, &id).map(|_| ())
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))?;
            Ok(bytes)
        })
    }
    fn release(&self, custody: Custody, pin: Pin) -> FutureResult<'_, ()> {
        Box::pin(async move {
            let holder = self.holder.clone();
            let policy = self.policy.clone();
            let operation = pin.operation.clone();
            self.db
                .write(16384, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Content);
                    pins::authorize(db, &holder, policy.as_ref(), &custody)?;
                    pins::release(db, &custody, &pin)
                })
                .await
                .map_err(|error| sql::error(error, operation))
        })
    }
    fn collect(&self, max_blocks: u32) -> FutureResult<'_, u32> {
        Box::pin(gc::collect(self, max_blocks))
    }
}
