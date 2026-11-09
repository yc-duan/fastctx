//! SQLite asset writer and its pre-transaction semantic preparation.
mod access;
mod admission;
mod basis;
mod bodies;
mod pending;
mod prepare;
pub(crate) use prepare::fingerprint as request_fingerprint;
mod rows;

use super::{
    Error,
    access::{AccessContext, AccessPolicy, Principal},
    authorize::MutationAuthorizer,
    mutation::{AtomicMutation, SubmissionReceipt},
};
use crate::{
    catalog::ObjectHead,
    cursor::{Holder, LocalSequence},
    identity::{HubId, ObjectId, ObjectKey, OperationId},
    manifest::BlockStore,
    registry::{Asset, Kind, REGISTRY, handlers::KindHandler},
};
use fastctx_storage::Database;
use std::{collections::BTreeMap, sync::Arc};

pub struct Config {
    pub database: Database,
    pub hub: HubId,
    pub holder: Holder,
    pub policy: Arc<dyn AccessPolicy>,
    pub blocks: Arc<dyn BlockStore>,
    pub handlers: Vec<Arc<dyn KindHandler>>,
    pub authorizers: Vec<Arc<dyn MutationAuthorizer>>,
    pub semantic_bytes: usize,
}
#[derive(Clone)]
pub struct SqliteBackend {
    db: Database,
    hub: HubId,
    holder: Holder,
    policy: Arc<dyn AccessPolicy>,
    blocks: Arc<dyn BlockStore>,
    handlers: Arc<BTreeMap<&'static str, Arc<dyn KindHandler>>>,
    authorizers: Arc<BTreeMap<&'static str, Arc<dyn MutationAuthorizer>>>,
    semantic_bytes: usize,
    semantic_budget: Arc<admission::Admission>,
    changed: tokio::sync::watch::Sender<LocalSequence>,
}
impl SqliteBackend {
    /// Service composition only. Installed handlers and semantic authorizers
    /// determine which registered kinds may accept user edits.
    pub fn new(config: Config) -> Result<Self, Error> {
        if config.semantic_bytes < fastctx_types::sync::BLOCK_BYTES as usize {
            return Err(Error::Invalid("semantic buffer budget is smaller than one block".into()));
        }
        if matches!(&config.holder,Holder::Hub {hub} if hub!=&config.hub) {
            return Err(Error::Denied("holder belongs to another hub".into()));
        }
        let semantic_budget = Arc::new(admission::Admission::new(config.semantic_bytes)?);
        let mut handlers = BTreeMap::new();
        for handler in config.handlers {
            REGISTRY.asset(handler.kind()).map_err(Error::Invalid)?;
            if handlers.insert(handler.kind(), handler).is_some() {
                return Err(Error::Invalid("kind handler installed twice".into()));
            }
        }
        let mut authorizers = BTreeMap::new();
        for authorizer in config.authorizers {
            if !handlers.contains_key(authorizer.kind()) || authorizers.insert(authorizer.kind(), authorizer).is_some()
            {
                return Err(Error::Invalid("semantic authorizer has no unique installed handler".into()));
            }
        }
        let local = config
            .database
            .read_now(rows::local)
            .map_err(|e| super::sql::error(e, OperationId::from_bytes([0; 32])))?;
        Ok(Self {
            db: config.database,
            hub: config.hub,
            holder: config.holder,
            policy: config.policy,
            blocks: config.blocks,
            handlers: Arc::new(handlers),
            authorizers: Arc::new(authorizers),
            semantic_bytes: config.semantic_bytes,

            semantic_budget,
            changed: tokio::sync::watch::channel(local).0,
        })
    }
    pub async fn get(
        &self,
        context: AccessContext,
        kind: Kind<Asset>,
        object: ObjectId,
    ) -> Result<Option<ObjectHead>, Error> {
        if context.scope.hub != self.hub {
            return Err(Error::Denied("requested object belongs to another hub".into()));
        }
        let policy = self.policy.clone();
        self.db
            .read(move |db| {
                access::read(db, policy.as_ref(), &context, kind.registered())?;
                let key = ObjectKey { scope: context.scope.clone(), object };
                access::allowed(policy.can_read_object(&context, kind.registered(), &key), context.observed_acl)?;
                let head = rows::get(db, &key)?;
                if head.as_ref().is_some_and(|head| head.kind != kind.registration().id) {
                    return Err(super::sql::invalid("requested object has another kind"));
                }
                Ok(head)
            })
            .await
            .map_err(|e| super::sql::error(e, OperationId::from_bytes([0; 32])))
    }
    /// Stages a local edit without allocating an authority version. The authority
    /// path invokes acceptance instead and never calls this replica-only method.
    pub async fn submit_pending(
        &self,
        principal: Principal,
        mutation: AtomicMutation,
    ) -> Result<SubmissionReceipt, Error> {
        if matches!(self.holder, Holder::Hub { .. }) {
            return Err(Error::Invalid("a hub must use its authority acceptance path".into()));
        }
        pending::submit(self, principal, mutation).await
    }
}
