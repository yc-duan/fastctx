//! The hub's account directories. Every scope description is signed into its
//! owner's directory stream as a record of its own, and devices discover the
//! scopes their accounts' directories describe.
mod discover;
mod rows;
mod stream;

pub use discover::Recipient;
pub(crate) use discover::feed_view;

use crate::{
    blocks::{SqliteBlocks, authority_supply, codec},
    catalog::{Authority, HeadEvidence, Lifecycle, ObjectHead},
    cursor::{Completeness, Holder, HoldingPeriod, Revision, VersionRef},
    identity::{AccountId, OperationId, ScopeKey},
    manifest::{BlockStore, ContentRef, Encoding},
    proof::{Bytes, ProofSigner},
    registry::REGISTRY,
    scope::{
        AccountIdentity, ScopeDescriptor, ScopeOwner, ScopeProof, ScopeSubject, ScopeVisibility, account_scope,
        device_scope, directory_object,
    },
    store::{
        Error, FutureResult,
        access::{AccessContext, Custody, HoldingUse, Principal},
        boundary::{self, Role},
        control::AuthorityRun,
        feed, sql,
    },
};
use fastctx_types::sync::conversation_identity::conversation_scope;
use std::sync::Arc;

/// What the hub decides about one account-owned scope. The directory assigns
/// the description's revision; the caller states the ACL generation, which
/// advances whenever a change affects who may read or keep the scope.
#[derive(Clone, Debug)]
pub struct ScopeDescription {
    pub scope: ScopeKey,
    pub owner: AccountIdentity,
    pub subject: ScopeSubject,
    pub visibility: ScopeVisibility,
    pub acl_revision: Revision,
    pub lifecycle: Lifecycle,
}

pub struct SqliteDirectory {
    blocks: SqliteBlocks,
    run: AuthorityRun,
    signer: Arc<dyn ProofSigner>,
    /// Descriptions are written one at a time, each against the head it read.
    writing: tokio::sync::Mutex<()>,
}

impl SqliteDirectory {
    /// `signer` carries the hub's service key; `blocks` holds this hub's content.
    pub fn new(blocks: SqliteBlocks, run: AuthorityRun, signer: Arc<dyn ProofSigner>) -> Self {
        Self { blocks, run, signer, writing: tokio::sync::Mutex::new(()) }
    }

    pub(crate) fn database(&self) -> &fastctx_storage::Database {
        self.blocks.database()
    }
    pub(crate) fn hub(&self) -> &crate::identity::HubId {
        self.run.hub()
    }

    /// Signs `description` into its owner's directory and returns the proof a
    /// device installs it from. The owner's directory opens with its own
    /// description first. Repeating an operation returns what it wrote; an
    /// unchanged description is not written again.
    pub fn publish(&self, description: ScopeDescription, operation: OperationId) -> FutureResult<'_, ScopeProof> {
        Box::pin(async move {
            let _writing = self.writing.lock().await;
            self.check(&description)?;
            let owner = description.owner.clone();
            let directory = ScopeKey { hub: self.run.hub().clone(), scope: account_scope(&owner.id) };
            let opening = self.open(&directory, &owner).await?;
            if opening && description.scope != directory {
                let own = ScopeDescription {
                    scope: directory.clone(),
                    owner: owner.clone(),
                    subject: ScopeSubject::Account,
                    visibility: ScopeVisibility::Current,
                    acl_revision: Revision(1),
                    lifecycle: Lifecycle::Live,
                };
                let seed = format!("fastctx-sync/directory-own/v1\0{}", directory.scope.as_str());
                let operation = OperationId::from_bytes(crate::identity::Digest::of(seed.as_bytes()).bytes());
                self.append(&directory, own, operation).await?;
            }
            self.append(&directory, description, operation).await
        })
    }

    /// The description this hub last signed for `scope`.
    pub fn current(&self, scope: ScopeKey) -> FutureResult<'_, Option<ScopeDescriptor>> {
        Box::pin(async move {
            self.blocks
                .database()
                .read(move |db| described(db, &scope))
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))
        })
    }

    /// The hub describes only scopes whose identity derives from their subject,
    /// under owners whose identity derives from their legacy account.
    fn check(&self, description: &ScopeDescription) -> Result<(), Error> {
        let hub = self.run.hub();
        let owner = &description.owner;
        if description.scope.hub != *hub || owner.id != AccountId::from_legacy(hub, &owner.name, owner.opened) {
            return Err(Error::Invalid("a description names another hub or an underived account".into()));
        }
        let derived = match &description.subject {
            ScopeSubject::Account => account_scope(&owner.id),
            ScopeSubject::Device { device } => device_scope(device),
            ScopeSubject::Conversation { conversation, .. } => conversation_scope(conversation),
            ScopeSubject::Project { .. } | ScopeSubject::Machine { .. } | ScopeSubject::Administrators => {
                return Err(Error::Unavailable("this hub does not describe that kind of scope yet".into()));
            }
        };
        if derived != description.scope.scope {
            return Err(Error::Invalid("a scope's identity must derive from its subject".into()));
        }
        Ok(())
    }

    /// Opens the owner's directory if it does not exist yet. True while the
    /// directory still lacks its own description.
    async fn open(&self, directory: &ScopeKey, owner: &AccountIdentity) -> Result<bool, Error> {
        let (run, signer, key, owner) = (self.run.clone(), self.signer.clone(), directory.clone(), owner.clone());
        self.blocks
            .database()
            .write(64 * 1024, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Authority);
                match rows::get(db, &key)? {
                    Some(row) => Ok(matches!(row.origin, rows::Described::Opening { .. })),
                    None => {
                        rows::open_directory(db, &run, &key, &owner)?;
                        stream::open(db, &run, signer.as_ref(), &key)?;
                        Ok(true)
                    }
                }
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))
    }

    async fn append(
        &self,
        directory: &ScopeKey,
        description: ScopeDescription,
        operation: OperationId,
    ) -> Result<ScopeProof, Error> {
        let worker = self.blocks.worker().clone();
        let (read_directory, described, replay) = (directory.clone(), description.scope.clone(), operation.clone());
        let (head, previous, row, recorded) = self
            .blocks
            .database()
            .read(move |db| {
                let head = stream::head(db, &read_directory)?
                    .ok_or_else(|| sql::invalid("an open directory has no stream"))?;
                let previous = rows::get(db, &described)?.map(|row| row.origin.descriptor()).transpose()?.flatten();
                let row = rows::get(db, &read_directory)?.ok_or_else(|| sql::invalid("the directory row vanished"))?;
                let recorded = stream::recorded(db, &head, &replay)?;
                Ok((head, previous, (row.acl, row.publication), recorded))
            })
            .await
            .map_err(|error| sql::error(error, worker.clone()))?;
        if let Some(batch) = recorded {
            let proof = self.recorded_proof(&head, batch).await?;
            let stored: ScopeDescriptor =
                serde_json::from_slice(&proof.payload.0).map_err(|error| Error::Invalid(error.to_string()))?;
            if !same(&stored, &description) {
                return Err(Error::Invalid("an operation was reused for another description".into()));
            }
            return Ok(proof);
        }
        if let Some(previous) = &previous
            && same(previous, &description)
        {
            return self.current_proof(&description.scope).await;
        }
        let revision = match &previous {
            Some(previous) if previous.revision.authority_epoch == *self.run.epoch() => {
                Revision(previous.revision.revision.0 + 1)
            }
            _ => Revision(1),
        };
        let descriptor = ScopeDescriptor {
            format: 1,
            scope: description.scope.clone(),
            owner: ScopeOwner::Account { identity: description.owner.clone() },
            subject: description.subject.clone(),
            visibility: description.visibility.clone(),
            revision: VersionRef { authority_epoch: self.run.epoch().clone(), revision },
            acl_revision: description.acl_revision,
            lifecycle: description.lifecycle,
        };
        descriptor.validate().map_err(Error::Invalid)?;
        if let Some(previous) = &previous {
            descriptor.validate_successor(previous).map_err(Error::Invalid)?;
        }
        let payload = serde_json::to_vec(&descriptor).map_err(|error| Error::Invalid(error.to_string()))?;
        let custody = self.custody(directory, row.0, row.1)?;
        let encoded = codec::encode(directory.clone(), payload.clone(), Encoding::Raw)?;
        let published =
            self.blocks.put(custody.clone(), operation.clone(), encoded.representation, encoded.bytes).await?;
        let root = ContentRef::from(published.content.clone());
        let supplies =
            authority_supply::prepare(&self.blocks, &operation, &head.stream, &[(custody.clone(), root.clone())])
                .await?;
        let (run, signer, blocks) = (self.run.clone(), self.signer.clone(), self.blocks.clone());
        let directory = directory.clone();
        let expected = head.sequence;
        self.blocks
            .database()
            .write(64 * 1024 + payload.len(), move |db| {
                let _boundary = boundary::enter_typed_write(Role::Authority);
                let head =
                    stream::head(db, &directory)?.ok_or_else(|| sql::invalid("the directory stream vanished"))?;
                if head.sequence != expected {
                    return Err(fastctx_storage::Error::with_kind(
                        fastctx_storage::ErrorKind::Busy,
                        "the directory moved while a description was prepared",
                    ));
                }
                let (batch, through, prefix) =
                    stream::append(db, signer.as_ref(), &head, &operation, &published.content)?;
                supplies.adopt(db, &blocks, &custody, &root)?;
                let proof = ScopeProof { epoch: head.confirmed.clone(), batch, payload: Bytes(payload) };
                rows::describe(db, &run, &directory, &descriptor, &proof)?;
                let acl = rows::get(db, &directory)?.ok_or_else(|| sql::invalid("the directory row vanished"))?.acl;
                let version = VersionRef { authority_epoch: run.epoch().clone(), revision: Revision(through.0) };
                let advanced = ObjectHead {
                    key: directory_object(&directory),
                    kind: stream::KIND.into(),
                    schema: 1,
                    acl_revision: acl,
                    authority: Authority::Hub,
                    evidence: HeadEvidence::Stream { epoch: head.confirmed.proof.id(), through, prefix_digest: prefix },
                    revision: version.clone(),
                    current_version: version,
                    content: None,
                    history: None,
                    lifecycle: Lifecycle::Live,
                    completeness: Completeness::MetadataOnly,
                };
                feed::publish(db, &operation, &advanced)?;
                Ok(proof)
            })
            .await
            .map_err(|error| sql::error(error, worker))
    }

    /// The hub's own custody of a directory's records.
    fn custody(&self, directory: &ScopeKey, acl: Revision, publication: Revision) -> Result<Custody, Error> {
        Ok(Custody {
            context: AccessContext { principal: Principal::LocalService, scope: directory.clone(), observed_acl: acl },
            kind: REGISTRY.asset(stream::KIND).map_err(Error::Invalid)?.registered(),
            object: directory_object(directory),
            purpose: HoldingUse::Replica,
            holding: HoldingPeriod {
                holder: Holder::Hub { hub: self.run.hub().clone() },
                scope: directory.clone(),
                acl_revision: acl,
                publication_epoch: publication,
            },
        })
    }

    async fn recorded_proof(
        &self,
        head: &stream::Stream,
        batch: crate::proof::SignedSourceBatch,
    ) -> Result<ScopeProof, Error> {
        let claimed: crate::catalog::SourceBatch =
            serde_json::from_slice(&batch.proof.data.0).map_err(|error| Error::Invalid(error.to_string()))?;
        let record = claimed.records.first().ok_or_else(|| Error::Invalid("a directory batch has no record".into()))?;
        let row = self
            .blocks
            .database()
            .read({
                let directory = head.scope.clone();
                move |db| rows::get(db, &directory)
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?
            .ok_or_else(|| Error::Invalid("the directory row vanished".into()))?;
        let custody = self.custody(&head.scope, row.acl, row.publication)?;
        let (payload, _permit) = self.blocks.read_granted(&custody, &record.payload)?;
        Ok(ScopeProof { epoch: head.confirmed.clone(), batch, payload: Bytes(payload) })
    }

    async fn current_proof(&self, scope: &ScopeKey) -> Result<ScopeProof, Error> {
        let key = scope.clone();
        let row = self
            .blocks
            .database()
            .read(move |db| rows::get(db, &key))
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        match row.map(|row| row.origin) {
            Some(rows::Described::Directory { proof }) => Ok(*proof),
            _ => Err(Error::Invalid("the described scope has no signed description".into())),
        }
    }
}

/// The description this hub last signed for `scope`, read in the caller's
/// transaction; None for a scope this hub does not describe.
pub(crate) fn described(
    db: &fastctx_storage::rusqlite::Connection,
    scope: &ScopeKey,
) -> fastctx_storage::Result<Option<ScopeDescriptor>> {
    Ok(rows::get(db, scope)?.map(|row| row.origin.descriptor()).transpose()?.flatten())
}

/// Whether a stored description says what `description` asks for.
fn same(stored: &ScopeDescriptor, description: &ScopeDescription) -> bool {
    stored.scope == description.scope
        && stored.owner == (ScopeOwner::Account { identity: description.owner.clone() })
        && stored.subject == description.subject
        && stored.visibility == description.visibility
        && stored.acl_revision == description.acl_revision
        && stored.lifecycle == description.lifecycle
}
