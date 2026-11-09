//! Service-composition entry points for engine-issued source steps. Business code
//! reaches them through SourceIngest, which also requires an installed authorizer.
use super::SqliteBlocks;
use crate::{
    blocks::{
        budget::Permit,
        steps::{self, Cancelled, ReconcileCursor},
    },
    cursor::Revision,
    identity::HostGeneration,
    manifest::{ContentBlock, ContentRef, ManifestReply, ManifestRoot},
    store::{
        Error, FutureResult,
        access::Custody,
        boundary::{self, Role},
        ingest::SourceTarget,
        source_step::{CancelProgress, SourceStep, SourceStepTarget},
        sql,
    },
};

impl SqliteBlocks {
    /// Issues the next step of `primary`, which must be at checkpoint `expected`.
    pub fn begin_step(
        &self,
        primary: SourceTarget,
        expected: Option<Revision>,
        generation: HostGeneration,
    ) -> FutureResult<'_, SourceStep> {
        Box::pin(async move {
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            self.db
                .write(65536, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Source);
                    steps::begin(db, &holder, policy.as_ref(), &primary, expected, &generation)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    pub fn bind_target(&self, step: SourceStep, target: SourceTarget) -> FutureResult<'_, SourceStepTarget> {
        Box::pin(async move {
            let policy = self.policy.clone();
            let holder = self.holder.clone();
            let operation = step.operation().clone();
            let (bound, step) = (target.clone(), step.clone());
            self.db
                .write(65536, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Source);
                    steps::bind(db, &holder, policy.as_ref(), &step, &bound)?;
                    Ok(SourceStepTarget::new(step, bound))
                })
                .await
                .map_err(|error| sql::error(error, operation))
        })
    }
    /// Cancels the step durably, then releases up to `max` of its staged pins.
    pub fn cancel_step(&self, step: SourceStep, max: u32) -> FutureResult<'_, CancelProgress> {
        Box::pin(async move {
            if max == 0 || max > 128 {
                return Err(Error::Invalid("step cleanup needs a page of 1 to 128 pins".into()));
            }
            let holder = self.holder.clone();
            let operation = step.operation().clone();
            let cancelled = self
                .db
                .write(128 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Source);
                    steps::cancel(db, &holder, &step, max)
                })
                .await
                .map_err(|error| sql::error(error, operation))?;
            Ok(match cancelled {
                Cancelled::Draining { released } => CancelProgress::Draining { released },
                Cancelled::Done => CancelProgress::Done,
                Cancelled::Committed { receipt } => CancelProgress::Committed { receipt },
            })
        })
    }
    /// One startup page over active steps; returns where the next page starts.
    pub fn reconcile_steps(
        &self,
        after: Option<ReconcileCursor>,
        max: u32,
    ) -> FutureResult<'_, Option<ReconcileCursor>> {
        Box::pin(async move {
            if max == 0 || max > 128 {
                return Err(Error::Invalid("step reconciliation needs a page of 1 to 128 steps".into()));
            }
            self.db
                .write(128 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Source);
                    steps::reconcile(db, after.as_ref(), max)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    /// Releases one page of pins staged by cancelled or committed steps.
    pub fn drain_steps(&self, max: u32) -> FutureResult<'_, u32> {
        Box::pin(async move {
            if max == 0 || max > 128 {
                return Err(Error::Invalid("step cleanup needs a page of 1 to 128 pins".into()));
            }
            self.db
                .write(128 * 1024, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Source);
                    steps::drain(db, max)
                })
                .await
                .map_err(|error| sql::error(error, self.worker.clone()))
        })
    }
    /// Decoded bytes of a block this step supplied for `custody`, for the source
    /// writer's semantic checks before it commits the step. The permit covers
    /// the bytes and must live as long as they do.
    pub(crate) fn read_supplied(
        &self,
        step: &SourceStep,
        custody: &Custody,
        block: &ContentBlock,
    ) -> Result<(Vec<u8>, Permit), Error> {
        let reference = ContentRef::from(block.clone());
        let supplied = self
            .db
            .read_now(|db| {
                Ok(super::pins::ready_for(db, step.operation(), custody, &reference)?
                    || super::access::has_grant(db, custody, &reference)?)
            })
            .map_err(|error| sql::error(error, self.worker.clone()))?;
        if !supplied {
            return Err(Error::Invalid("a source record payload was not supplied by its step".into()));
        }
        super::metadata_work::payload_bytes(self, block, 0)?
            .ok_or_else(|| Error::Invalid("a source record payload is not stored".into()))
    }
    /// Decoded bytes of a block already granted to `custody`'s object, such as
    /// a descriptor an earlier step committed. Source-private uploads stay unreadable.
    pub(crate) fn read_granted(&self, custody: &Custody, block: &ContentBlock) -> Result<(Vec<u8>, Permit), Error> {
        let reference = ContentRef::from(block.clone());
        self.db
            .read_now(|db| {
                super::pins::authorize(db, &self.holder, self.policy.as_ref(), custody)?;
                if !super::access::has_grant(db, custody, &reference)? {
                    return Err(sql::invalid("the block is not granted to this object"));
                }
                Ok(())
            })
            .map_err(|error| sql::error(error, self.worker.clone()))?;
        super::metadata_work::payload_bytes(self, block, 0)?
            .ok_or_else(|| Error::Unavailable("the granted block is not stored".into()))
    }
    /// One manifest page already granted to `custody`'s object.
    pub(crate) fn manifest_granted(&self, custody: &Custody, root: &ManifestRoot) -> Result<ManifestReply, Error> {
        let _lease = self.leases.read(&root.0)?;
        self.db
            .read_now(|db| {
                super::pins::authorize(db, &self.holder, self.policy.as_ref(), custody)?;
                if !super::access::has_grant(db, custody, &ContentRef::Manifest { root: root.clone() })? {
                    return Err(sql::invalid("the page is not granted to this object"));
                }
                super::pages::get(db, root)
            })
            .map_err(|error| sql::error(error, self.worker.clone()))
    }
}
