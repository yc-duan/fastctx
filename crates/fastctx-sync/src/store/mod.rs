//! Typed handles keep authority writes, local pending edits and runtime state apart.

pub mod access;
pub mod audit;
pub mod authority;
pub mod authorize;
pub mod boundary;
pub mod control;
pub mod directory;
pub mod engine;
pub(crate) mod epochs;
pub(crate) mod feed;
pub mod format_upgrade;
pub mod ingest;
pub mod ingest_effects;
pub mod ingest_memory;
pub(crate) mod local_head;
pub mod mutation;
pub mod projection;
pub mod replica;
pub mod roots;
pub mod schema;
pub mod scopes;
pub mod source_authorize;
pub mod source_step;
pub mod sources;
pub(crate) mod sql;
pub mod timelines;

use crate::{
    catalog::ObjectHead,
    cursor::{HoldingPeriod, LocalSequence, Revision, VersionRef},
    identity::{ObjectId, OperationId},
    registry::{Asset, Derived, Kind, Runtime},
};
use access::{AccessContext, Principal};
use mutation::{
    AtomicMutation, AuthorityReceipt, EffectiveObject, OperationReceipt, Origin, PendingMutation, ProjectionChange,
    ProposedObject, RuntimeChange, Submission, SubmissionMode, SubmissionReceipt,
};
use std::{future::Future, pin::Pin, sync::Arc};

pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Denied(String),
    Conflict { current: VersionRef },
    LocalConflict { current: Revision },
    Revoked,
    Busy,
    Full,
    Corrupt(String),
    Unavailable(String),
    OutcomeUnknown { operation: OperationId },
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug)]
pub struct PageLimit {
    pub items: u32,
    pub bytes: u32,
}
impl Default for PageLimit {
    fn default() -> Self {
        Self { items: 100, bytes: fastctx_types::sync::PAGE_BYTES }
    }
}
#[derive(Clone, Debug)]
pub struct ReadRequest {
    pub after: Option<ObjectId>,
    pub through: Option<LocalSequence>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug)]
pub struct ReadPage<T> {
    pub through: LocalSequence,
    pub items: Vec<T>,
    pub next: Option<ObjectId>,
}
#[derive(Clone, Debug)]
pub struct Subscribe {
    pub after: LocalSequence,
    pub objects: Vec<ObjectId>,
}
#[derive(Clone, Debug)]
pub struct Change {
    pub local: LocalSequence,
    pub object: ObjectId,
    pub revision: Option<VersionRef>,
    pub origin: Origin,
}
pub trait Subscription: Send {
    fn next(&mut self, limit: PageLimit) -> FutureResult<'_, Vec<Change>>;
}

/// Runtime and projection versions are assigned by their local owner. This
/// receipt is never returned as an accepted asset revision for a pending edit.
#[derive(Clone, Debug)]
pub struct LocalCommitReceipt {
    pub operation: OperationId,
    pub local: LocalSequence,
    pub revision: Revision,
}
#[derive(Clone, Debug)]
pub struct LocalRecord {
    pub revision: Revision,
    pub body: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct RevocationStarted {
    pub invalidated: HoldingPeriod,
    pub cleanup: OperationId,
}

pub trait Backend: Send + Sync {
    fn get(&self, context: AccessContext, kind: Kind<Asset>, object: ObjectId) -> FutureResult<'_, Option<ObjectHead>>;
    fn effective(
        &self,
        context: AccessContext,
        kind: Kind<Asset>,
        object: ObjectId,
    ) -> FutureResult<'_, EffectiveObject>;
    fn get_runtime(
        &self,
        context: AccessContext,
        kind: Kind<Runtime>,
        key: String,
    ) -> FutureResult<'_, Option<LocalRecord>>;
    fn get_projection(
        &self,
        context: AccessContext,
        kind: Kind<Derived>,
        object: ObjectId,
    ) -> FutureResult<'_, Option<projection::ProjectionRecord>>;
    fn subscribe_projection(
        &self,
        context: AccessContext,
        kind: Kind<Derived>,
        request: Subscribe,
    ) -> FutureResult<'_, Box<dyn projection::ProjectionSubscription>>;
    fn read(
        &self,
        context: AccessContext,
        kind: Kind<Asset>,
        request: ReadRequest,
    ) -> FutureResult<'_, ReadPage<ObjectHead>>;
    fn subscribe(
        &self,
        context: AccessContext,
        kind: Kind<Asset>,
        request: Subscribe,
    ) -> FutureResult<'_, Box<dyn Subscription>>;
    /// Persists pending objects, their pins and outbox together; no authority
    /// revision is allocated here unless this service is the confirmed authority.
    fn submit_atomic(&self, principal: Principal, mutation: AtomicMutation) -> FutureResult<'_, SubmissionReceipt>;
    /// Receipt lookup precedes rebase or expected-version evaluation on retry.
    fn receipt(&self, principal: Principal, operation: OperationId) -> FutureResult<'_, Option<OperationReceipt>>;
    fn pending(
        &self,
        context: AccessContext,
        after: LocalSequence,
        limit: PageLimit,
    ) -> FutureResult<'_, Vec<PendingMutation>>;
    /// Only the object's registered authority installs accepted versions.
    fn accept(&self, principal: Principal, mutation: AtomicMutation) -> FutureResult<'_, AuthorityReceipt>;
    /// Verifies an original acceptance before installing old-epoch versions.
    /// Missing receipts without this proof remain OutcomeUnknown after recovery.
    fn restore_accepted(
        &self,
        principal: Principal,
        proof: crate::proof::SignedAcceptanceFragment,
    ) -> FutureResult<'_, AuthorityReceipt>;
    fn write_runtime(
        &self,
        context: AccessContext,
        kind: Kind<Runtime>,
        change: RuntimeChange,
    ) -> FutureResult<'_, LocalCommitReceipt>;
    fn write_projection(
        &self,
        context: AccessContext,
        kind: Kind<Derived>,
        change: ProjectionChange,
    ) -> FutureResult<'_, LocalCommitReceipt>;
    /// Logical invalidation, not a physical exposure-cleanup receipt.
    fn revoke(&self, holding: HoldingPeriod) -> FutureResult<'_, RevocationStarted>;
}

pub struct Stores<B> {
    backend: Arc<B>,
}
impl<B: Backend> Stores<B> {
    pub fn new(backend: B) -> Self {
        Self { backend: Arc::new(backend) }
    }
    pub fn assets(&self, context: AccessContext, kind: Kind<Asset>) -> AssetStore<B> {
        AssetStore { backend: self.backend.clone(), context, kind }
    }
    pub fn atomic(&self, principal: Principal) -> AtomicStore<B> {
        AtomicStore { backend: self.backend.clone(), principal }
    }
    pub fn runtime(&self, context: AccessContext, kind: Kind<Runtime>) -> RuntimeStore<B> {
        RuntimeStore { backend: self.backend.clone(), context, kind }
    }
    pub fn projection(&self, context: AccessContext, kind: Kind<Derived>) -> ProjectionStore<B> {
        ProjectionStore { backend: self.backend.clone(), context, kind }
    }
}

pub struct AtomicStore<B> {
    backend: Arc<B>,
    principal: Principal,
}
impl<B: Backend> AtomicStore<B> {
    pub fn submit(&self, request: Submission) -> FutureResult<'_, SubmissionReceipt> {
        let mutation = match AtomicMutation::new(request) {
            Ok(mutation) => mutation,
            Err(error) => return Box::pin(async { Err(error) }),
        };
        self.backend.submit_atomic(self.principal.clone(), mutation)
    }
    pub fn receipt(&self, operation: OperationId) -> FutureResult<'_, Option<OperationReceipt>> {
        self.backend.receipt(self.principal.clone(), operation)
    }
}

pub struct AssetStore<B> {
    backend: Arc<B>,
    context: AccessContext,
    kind: Kind<Asset>,
}
impl<B: Backend> AssetStore<B> {
    pub fn get(&self, object: ObjectId) -> FutureResult<'_, Option<ObjectHead>> {
        self.backend.get(self.context.clone(), self.kind, object)
    }
    pub fn effective(&self, object: ObjectId) -> FutureResult<'_, EffectiveObject> {
        self.backend.effective(self.context.clone(), self.kind, object)
    }
    pub fn read(&self, request: ReadRequest) -> FutureResult<'_, ReadPage<ObjectHead>> {
        self.backend.read(self.context.clone(), self.kind, request)
    }
    pub fn subscribe(&self, request: Subscribe) -> FutureResult<'_, Box<dyn Subscription>> {
        self.backend.subscribe(self.context.clone(), self.kind, request)
    }
    pub fn submit(
        &self,
        operation: OperationId,
        object: ProposedObject,
        mode: SubmissionMode,
    ) -> FutureResult<'_, SubmissionReceipt> {
        if object.key.scope != self.context.scope || object.kind != self.kind.registration().id {
            return Box::pin(async { Err(Error::Invalid("the proposal differs from its bound handle".into())) });
        }
        let origin = match &self.context.principal {
            Principal::Account { device, .. } | Principal::Agent { device, .. } => device.clone(),
            Principal::LocalService => {
                return Box::pin(async { Err(Error::Denied("local maintenance cannot impersonate an editor".into())) });
            }
        };
        let request = Submission { operation, origin, mode, objects: vec![object] };
        let mutation = match AtomicMutation::new(request) {
            Ok(mutation) => mutation,
            Err(error) => return Box::pin(async { Err(error) }),
        };
        self.backend.submit_atomic(self.context.principal.clone(), mutation)
    }
}

pub struct RuntimeStore<B> {
    backend: Arc<B>,
    context: AccessContext,
    kind: Kind<Runtime>,
}
impl<B: Backend> RuntimeStore<B> {
    pub fn get(&self, key: String) -> FutureResult<'_, Option<LocalRecord>> {
        self.backend.get_runtime(self.context.clone(), self.kind, key)
    }
    pub fn write(&self, change: RuntimeChange) -> FutureResult<'_, LocalCommitReceipt> {
        self.backend.write_runtime(self.context.clone(), self.kind, change)
    }
}
pub struct ProjectionStore<B> {
    backend: Arc<B>,
    context: AccessContext,
    kind: Kind<Derived>,
}
impl<B: Backend> ProjectionStore<B> {
    pub fn subscribe(&self, request: Subscribe) -> FutureResult<'_, Box<dyn projection::ProjectionSubscription>> {
        self.backend.subscribe_projection(self.context.clone(), self.kind, request)
    }
    pub fn get(&self, object: ObjectId) -> FutureResult<'_, Option<projection::ProjectionRecord>> {
        self.backend.get_projection(self.context.clone(), self.kind, object)
    }
    pub fn replace(&self, change: ProjectionChange) -> FutureResult<'_, LocalCommitReceipt> {
        self.backend.write_projection(self.context.clone(), self.kind, change)
    }
}
