//! Durable usage holds are shared by ingestion, timeline fences and materialization.
use super::{Error, FutureResult, access::Custody};
use crate::{
    cursor::{Completeness, HoldingPeriod, Revision},
    identity::{ObjectKey, OperationId, RootLeaseId, SourceId},
    manifest::ContentRef,
    registry::REGISTRY,
};

/// These local uses never advertise a new replicated asset or grant body reads.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "use", rename_all = "snake_case")]
pub enum RootUse {
    Checkpoint { source: SourceId },
    Timeline { projection: Revision },
    Runtime { kind: String, record_key: String },
}
impl RootUse {
    pub fn validate(&self) -> Result<(), Error> {
        if let Self::Runtime { kind, record_key } = self {
            REGISTRY.runtime(kind).map_err(Error::Invalid)?;
            if record_key.is_empty() || record_key.len() > 1024 {
                return Err(Error::Invalid("runtime root owner key exceeds its bound".into()));
            }
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct AcquireRoot {
    pub lease: RootLeaseId,
    pub custody: Custody,
    pub root: ContentRef,
    pub usage: RootUse,
    /// Only runtime usage expires. Asset retention is independent of this lease.
    pub expires_at_millis: Option<i64>,
}
impl AcquireRoot {
    pub fn validate(&self) -> Result<(), Error> {
        self.usage.validate()?;
        self.root.validate().map_err(Error::Invalid)?;
        match self.usage {
            RootUse::Checkpoint { .. } if self.expires_at_millis.is_some() => {
                return Err(Error::Invalid("checkpoint state lives until advancement or explicit discard".into()));
            }
            RootUse::Timeline { projection }
                if projection.0 == 0
                    || self.expires_at_millis.is_none()
                    || !matches!(self.root, ContentRef::Manifest { .. }) =>
            {
                return Err(Error::Invalid("timeline usage needs a manifest, projection and expiry".into()));
            }
            _ => {}
        }
        if self.root.id().domain != self.custody.object.scope
            || self.custody.context.scope != self.custody.object.scope
            || self.custody.holding.scope != self.custody.object.scope
        {
            return Err(Error::Invalid("root usage crosses its object custody scope".into()));
        }
        Ok(())
    }
}
/// This receipt identifies a persisted use. Release and reads recheck the saved
/// lease, owner, kind, root, usage and holding rather than trusting these fields.
#[derive(Clone, Debug)]
pub struct RootLease {
    pub id: RootLeaseId,
    pub operation: OperationId,
    pub object: ObjectKey,
    pub kind: String,
    pub root: ContentRef,
    pub usage: RootUse,
    pub holding: HoldingPeriod,
    pub generation: Revision,
    pub expires_at_millis: Option<i64>,
}
#[derive(Clone)]
pub enum RootChange {
    Acquire(Box<AcquireRoot>),
    Release { custody: Box<Custody>, lease: Box<RootLease> },
}
#[derive(Clone, Debug)]
pub enum RootAcquired {
    Held { lease: Box<RootLease> },
    Pending { completeness: Completeness },
}

pub(crate) mod sealed {
    pub trait RootWriter {}
}

/// Service composition supplies a restricted handle. Every acquisition checks
/// exact object custody and a complete root closure in the same transaction as
/// its hold. The writer uses RootChange for joint commits with runtime intents;
/// neither path re-uploads bytes or borrows the process-local IO lease.
pub trait RootStore: sealed::RootWriter + Send + Sync {
    fn acquire_root(&self, operation: OperationId, request: AcquireRoot) -> FutureResult<'_, RootAcquired>;
    /// Releasing the same persisted generation twice succeeds without touching
    /// another use. Revocation cleanup has its own private writer authority.
    fn release_root(&self, custody: Custody, lease: RootLease) -> FutureResult<'_, ()>;
}
