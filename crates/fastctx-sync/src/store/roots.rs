//! Durable usage holds are shared by ingestion, timeline fences and materialization.
use super::{Error, FutureResult, access::Custody};
use crate::{
    cursor::{Completeness, HoldingPeriod, Revision},
    identity::{ObjectKey, OperationId, RootLeaseId, SourceId},
    manifest::ContentRef,
    registry::REGISTRY,
};
use std::{future::Future, pin::Pin};

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
/// Each reuse basis is checked and protected before any traversal begins.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SupplyBasis {
    Published { root: ContentRef },
    Held { lease: Box<RootLease> },
}

#[derive(Clone)]
pub struct AcquireRoot {
    pub lease: RootLeaseId,
    pub custody: Custody,
    pub root: ContentRef,
    pub usage: RootUse,
    pub bases: Vec<SupplyBasis>,
    /// Timeline and runtime holds may expire. Asset retention is independent.
    pub expires_at_millis: Option<i64>,
}
impl AcquireRoot {
    pub fn validate(&self) -> Result<(), Error> {
        self.usage.validate()?;
        self.root.validate().map_err(Error::Invalid)?;
        if self.bases.len() > 8 || matches!(self.usage, RootUse::Timeline { .. }) && !self.bases.is_empty() {
            return Err(Error::Invalid("root usage exceeds its declared reuse bases".into()));
        }
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
/// lease, owner, kind, root and usage rather than trusting these fields. Release
/// checks the actual holder but does not require the original grant to be current.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

/// A timeline head can move between choosing its root and acquiring its hold.
/// Conversion preserves the ordinary invalid-request behavior for callers that
/// do not re-read the committed head to establish that this was a race.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcquireRootError {
    TimelineUnbound,
    Store(Error),
}
impl std::fmt::Display for AcquireRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimelineUnbound => f.write_str("timeline root differs from its committed projection or live fence"),
            Self::Store(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for AcquireRootError {}
impl From<Error> for AcquireRootError {
    fn from(error: Error) -> Self {
        Self::Store(error)
    }
}
impl From<AcquireRootError> for Error {
    fn from(error: AcquireRootError) -> Self {
        match error {
            AcquireRootError::TimelineUnbound => Self::Invalid(error.to_string()),
            AcquireRootError::Store(error) => error,
        }
    }
}
pub type AcquireRootResult<'a> = Pin<Box<dyn Future<Output = Result<RootAcquired, AcquireRootError>> + Send + 'a>>;

pub(crate) mod sealed {
    pub trait RootWriter {}
}

/// Service composition supplies a restricted handle. Every acquisition checks
/// exact object custody in the same transaction as its hold. Checkpoint and
/// runtime uses require a complete, supplied closure; a timeline fence holds its
/// verified authorized root and available descendants while missing pages load.
/// The writer uses RootChange for joint commits with runtime intents;
/// neither path re-uploads bytes or borrows the process-local IO lease.
pub trait RootStore: sealed::RootWriter + Send + Sync {
    fn acquire_root(&self, operation: OperationId, request: AcquireRoot) -> AcquireRootResult<'_>;
    /// Releasing the same persisted generation twice succeeds without touching
    /// another use. Old holding periods remain releasable by the same holder;
    /// reacquisition still requires current custody. Revocation cleanup has its
    /// own private writer authority.
    fn release_root(&self, custody: Custody, lease: RootLease) -> FutureResult<'_, ()>;
}
