//! Every durable kind declares its ownership, mutation and lifetime contract.

pub mod handlers;
mod kinds;
pub mod legacy;
pub mod text;
pub static REGISTRY: Registry = Registry::new(kinds::KINDS);
pub use fastctx_types::sync::catalog::DataClass;
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Foundation,
    Ingestion,
    Projection,
    Assets,
    Client,
    Migration,
    Maintenance,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopePolicy {
    Account,
    Conversation,
    Project,
    Machine,
    Device,
    Local,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityRule {
    Hub,
    SourceDevice,
    LoginDevice,
    Local,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictRule {
    SingleWriter,
    Expected,
    TextMerge,
    Yrs,
    Rebuild,
    LocalOwner,
    LocalPending,
    MigrationSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retention {
    UntilDeleted,
    UntilResolved,
    Rebuildable,
    Temporary,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lightweight {
    Complete,
    Recent,
    Metadata,
    LocalOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Json,
    SourceRecords,
    Utf8,
    YrsV1,
    ManifestV1,
    Bytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionState {
    Confirmed,
    PendingUser,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportPolicy {
    GenericRootClosure,
    Handler,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletePolicy {
    GenericTombstone,
    HandlerWithTombstone,
}

#[derive(Clone, Copy, Debug)]
pub struct Registration {
    pub id: &'static str,
    pub schema: u16,
    pub class: DataClass,
    pub scope: ScopePolicy,
    pub authority: AuthorityRule,
    pub conflict: ConflictRule,
    pub retention: Retention,
    pub mobile: Lightweight,
    pub owner: &'static str,
    /// The required handler has this same ID and supplies migration and both projections.
    pub handler: &'static str,
    pub codec: Codec,
    pub exportable: bool,
    pub export: ExportPolicy,
    pub delete: DeletePolicy,
    pub assisted_host: bool,
    /// Only facts and output produced by this job's verified executor.
    pub job_source: bool,
    /// Only artifacts ingested from this machine's registered workspace.
    pub workspace_source: bool,
    pub decision: DecisionState,
    pub stage: Stage,
}

pub enum Asset {}
pub enum Runtime {}
pub enum Derived {}
pub enum Scratch {}

/// Constructed only after a registered kind's class has been checked.
pub struct Kind<C> {
    registration: &'static Registration,
    class: PhantomData<C>,
}
impl<C> Copy for Kind<C> {}
impl<C> Clone for Kind<C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C> Kind<C> {
    pub fn registration(&self) -> &'static Registration {
        self.registration
    }
    pub fn registered(&self) -> RegisteredKind {
        RegisteredKind(self.registration)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RegisteredKind(&'static Registration);
impl RegisteredKind {
    pub fn registration(&self) -> &'static Registration {
        self.0
    }
}

pub struct Registry {
    registrations: &'static [Registration],
}
impl Registry {
    pub(crate) const fn new(registrations: &'static [Registration]) -> Self {
        Self { registrations }
    }
    pub fn entries(&self) -> &'static [Registration] {
        self.registrations
    }
    fn kind<C>(&self, id: &str, class: DataClass) -> Result<Kind<C>, String> {
        let found = self
            .registrations
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("unregistered data kind: {id}"))?;
        if found.decision == DecisionState::PendingUser {
            return Err(format!("the retention and replication policy for {id} is not decided"));
        }
        if found.class != class {
            return Err(format!("data kind {id} has a different storage class"));
        }
        Ok(Kind { registration: found, class: PhantomData })
    }
    pub fn asset(&self, id: &str) -> Result<Kind<Asset>, String> {
        self.kind(id, DataClass::Asset)
    }
    pub fn runtime(&self, id: &str) -> Result<Kind<Runtime>, String> {
        self.kind(id, DataClass::Runtime)
    }
    pub fn projection(&self, id: &str) -> Result<Kind<Derived>, String> {
        self.kind(id, DataClass::Derived)
    }
    pub fn scratch(&self, id: &str) -> Result<Kind<Scratch>, String> {
        self.kind(id, DataClass::Scratch)
    }
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::BTreeSet::new();
        for entry in self.registrations {
            if entry.id.is_empty() || !ids.insert(entry.id) {
                return Err("duplicate or empty data kind".into());
            }
            if entry.class == DataClass::Asset && entry.retention != Retention::UntilDeleted {
                return Err(format!("asset {} has an automatic eviction policy", entry.id));
            }
            if (entry.job_source || entry.workspace_source)
                && (entry.class != DataClass::Asset
                    || entry.scope != ScopePolicy::Conversation
                    || entry.authority != AuthorityRule::SourceDevice
                    || entry.conflict != ConflictRule::SingleWriter)
            {
                return Err(format!("source custody for {} lacks a conversation source authority", entry.id));
            }
        }
        Ok(())
    }
}
