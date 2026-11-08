//! Per-operation evidence checked before releasing the writer's savepoint.
//!
//! Storage hooks, not caller-supplied messages, populate this ledger. It covers
//! all object effects of one operation, even when many operations share a commit.

use super::Error;
use crate::{
    cursor::VersionRef,
    identity::{Digest, ObjectKey, OperationId},
};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Effect {
    pub operation: OperationId,
    pub object: ObjectKey,
    pub request_hash: Digest,
    /// None identifies a pending edit, never an invented authority version.
    pub revision: Option<VersionRef>,
}

#[derive(Default)]
pub struct OperationAudit {
    changed: HashSet<Effect>,
    journaled: HashSet<Effect>,
    outbound: HashSet<Effect>,
    local: HashSet<Effect>,
    poisoned: bool,
}

impl OperationAudit {
    /// A caught hook panic makes absence of evidence a refusal, not success.
    pub fn poison(&mut self) {
        self.poisoned = true;
    }
    pub fn authoritative_row(&mut self, effect: Effect, produces_outbound: bool) {
        self.changed.insert(effect.clone());
        if produces_outbound {
            self.local.insert(effect);
        }
    }
    pub fn local_change(&mut self, effect: Effect) {
        self.journaled.insert(effect);
    }
    pub fn outbox(&mut self, effect: Effect) {
        self.outbound.insert(effect);
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Invalid("the operation audit did not complete".into()));
        }
        if !self.changed.is_subset(&self.journaled) {
            return Err(Error::Invalid("an authoritative change has no durable changefeed entry".into()));
        }
        if !self.local.is_subset(&self.outbound) {
            return Err(Error::Invalid("a local asset mutation has no durable replication outbox".into()));
        }
        Ok(())
    }
}

/// DDL and ATTACH need authorization in addition to row pre-update hooks.
/// Installed table names are compared with the closed schema before serving.
pub trait SchemaPolicy: Send + Sync {
    fn permits_table(&self, database: &str, table: &str) -> bool;
    fn permits_schema_change(&self, migration_component: Option<&str>) -> bool;
    fn permits_attach(&self) -> bool;
}
