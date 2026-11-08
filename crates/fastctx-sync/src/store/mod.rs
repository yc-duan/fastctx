//! Typed service interfaces. Backend implementations must enforce authorization
//! again inside writes and immediately before publishing data to a reader.

pub mod access;
pub mod audit;
pub mod ingest;
pub mod mutation;
pub mod schema;

use crate::{
    catalog::ObjectHead,
    cursor::{LocalSequence, Revision},
    identity::{ObjectId, OperationId, ScopeKey},
    registry::{Asset, Derived, Kind, Runtime},
};
use access::AccessContext;
use mutation::{AssetChange, Expected, Origin, ProjectionChange, RuntimeChange};
use std::{future::Future, pin::Pin, sync::Arc};

pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Denied(String),
    Conflict {
        current: Revision,
    },
    Revoked,
    Busy,
    Full,
    Corrupt(String),
    Unavailable(String),
    /// Retry with the original operation ID; do not invent a second mutation.
    OutcomeUnknown {
        operation: OperationId,
    },
}
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
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
    pub revision: Revision,
    pub origin: Origin,
}

/// Wakes are hints. `next` replays persisted changes after the subscriber's
/// cursor and never treats a lagged in-memory queue as an empty history.
pub trait Subscription: Send {
    fn next(&mut self, limit: PageLimit) -> FutureResult<'_, Vec<Change>>;
}

/// Constructed by storage only after a durable commit. The hub cursor is absent
/// until a hub has accepted the change; local sequence numbers cannot fill it.
#[derive(Clone, Debug)]
pub struct CommitReceipt {
    pub operation: OperationId,
    pub local: LocalSequence,
    pub revision: Revision,
    pub hub: Option<crate::cursor::ScopeCursor>,
}

#[derive(Clone, Debug)]
pub struct LocalRecord {
    pub revision: Revision,
    pub body: Vec<u8>,
}

/// Services install one backend at their composition root. No implementation
/// may expose a raw connection through the typed handles it returns.
pub trait Backend: Send + Sync {
    fn get(&self, context: AccessContext, kind: Kind<Asset>, object: ObjectId) -> FutureResult<'_, Option<ObjectHead>>;
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
    ) -> FutureResult<'_, Option<LocalRecord>>;
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
    fn commit(
        &self,
        context: AccessContext,
        kind: Kind<Asset>,
        mutation: mutation::Mutation,
    ) -> FutureResult<'_, CommitReceipt>;
    fn write_runtime(
        &self,
        context: AccessContext,
        kind: Kind<Runtime>,
        change: RuntimeChange,
    ) -> FutureResult<'_, CommitReceipt>;
    fn write_projection(
        &self,
        context: AccessContext,
        kind: Kind<Derived>,
        change: ProjectionChange,
    ) -> FutureResult<'_, CommitReceipt>;
    fn revoke(&self, scope: ScopeKey, revision: Revision) -> FutureResult<'_, ()>;
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
    pub fn runtime(&self, context: AccessContext, kind: Kind<Runtime>) -> RuntimeStore<B> {
        RuntimeStore { backend: self.backend.clone(), context, kind }
    }
    pub fn projection(&self, context: AccessContext, kind: Kind<Derived>) -> ProjectionStore<B> {
        ProjectionStore { backend: self.backend.clone(), context, kind }
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
    pub fn read(&self, request: ReadRequest) -> FutureResult<'_, ReadPage<ObjectHead>> {
        self.backend.read(self.context.clone(), self.kind, request)
    }
    pub fn subscribe(&self, request: Subscribe) -> FutureResult<'_, Box<dyn Subscription>> {
        self.backend.subscribe(self.context.clone(), self.kind, request)
    }
    pub fn commit(
        &self,
        operation: OperationId,
        object: ObjectId,
        expected: Expected,
        change: AssetChange,
    ) -> FutureResult<'_, CommitReceipt> {
        self.backend.commit(self.context.clone(), self.kind, mutation::Mutation { operation, object, expected, change })
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
    pub fn write(&self, change: RuntimeChange) -> FutureResult<'_, CommitReceipt> {
        self.backend.write_runtime(self.context.clone(), self.kind, change)
    }
}

pub struct ProjectionStore<B> {
    backend: Arc<B>,
    context: AccessContext,
    kind: Kind<Derived>,
}
impl<B: Backend> ProjectionStore<B> {
    pub fn get(&self, object: ObjectId) -> FutureResult<'_, Option<LocalRecord>> {
        self.backend.get_projection(self.context.clone(), self.kind, object)
    }
    pub fn replace(&self, change: ProjectionChange) -> FutureResult<'_, CommitReceipt> {
        self.backend.write_projection(self.context.clone(), self.kind, change)
    }
}
