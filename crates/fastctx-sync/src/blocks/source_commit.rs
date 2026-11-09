//! The content side of a source commit. Inside the source writer's transaction it
//! adopts only completed supply checks, swaps the checkpoint's state hold and
//! closes the step.
use super::{
    access,
    manifest::Reference,
    pages, pins, root_holds,
    source_supply::SourceSupplies,
    steps,
    supply::{self, SupplyWriter},
};
use crate::{
    cursor::Holder,
    identity::{Digest, RootLeaseId},
    manifest::ContentRef,
    store::{
        access::{AccessPolicy, Custody},
        ingest::{IngestReceipt, SourceTarget},
        roots::{AcquireRoot, RootAcquired, RootChange, RootLease, RootUse},
        source_step::SourceStep,
        sql,
    },
};
use fastctx_storage::{Result, rusqlite::Connection};

pub(crate) use steps::StepState;
pub(crate) fn step_state(db: &Connection, step: &SourceStep) -> Result<StepState> {
    steps::state(db, step)
}
pub(crate) fn require_bound(db: &Connection, step: &SourceStep, target: &SourceTarget) -> Result<()> {
    steps::require_bound(db, step, target)
}
pub(crate) fn consume(db: &Connection, step: &SourceStep, receipt: &IngestReceipt) -> Result<()> {
    steps::consume(db, step, receipt)
}
pub(crate) fn authorize(db: &Connection, holder: &Holder, policy: &dyn AccessPolicy, custody: &Custody) -> Result<()> {
    pins::authorize(db, holder, policy, custody)
}
pub(crate) fn release(db: &Connection, holder: &Holder, custody: &Custody, lease: &RootLease) -> Result<()> {
    root_holds::release(db, holder, custody, lease)
}

pub(crate) struct ContentCommit<'tx> {
    db: &'tx Connection,
    holder: &'tx Holder,
    writer: SupplyWriter<'tx>,
    supplies: &'tx SourceSupplies,
}
impl<'tx> ContentCommit<'tx> {
    pub(crate) fn new(
        db: &'tx Connection,
        holder: &'tx Holder,
        policy: &'tx dyn AccessPolicy,
        supplies: &'tx SourceSupplies,
        now: i64,
    ) -> Result<Self> {
        Ok(Self { db, holder, writer: supply::writer(db, holder, policy, now)?, supplies })
    }
    /// Records keep their payload and declared roots in the enclosing transaction.
    pub(crate) fn adopt(&self, target: &SourceTarget, root: &ContentRef) -> Result<()> {
        let ticket = self.supplies.record(target, root)?;
        let ready = self.writer.recheck(&target.custody, ticket)?;
        self.writer.retain_supplied(ready)?;
        // A descriptor's own references are readable as soon as it is committed;
        // deeper levels expand through the content worker as usual.
        if let ContentRef::Manifest { root: page } = root {
            let custody = &target.custody;
            let references: Vec<_> = pages::references(self.db, &page.0.domain, &page.0.digest)?
                .into_iter()
                .map(|child| match child {
                    ContentRef::Single { block, decoded_bytes } => Reference::Block { id: block, decoded_bytes },
                    ContentRef::Manifest { root } => Reference::Manifest { root },
                })
                .collect();
            let kind = custody.kind.registration().id;
            access::expand(self.db, &custody.object, kind, page, &references, &custody.holding)?;
        }
        Ok(())
    }
    /// The parser's state hold is installed only with the checkpoint it protects.
    pub(crate) fn hold_checkpoint(&self, step: &SourceStep, root: &ContentRef) -> Result<RootLease> {
        let primary = step.primary();
        let digest = Digest::of(format!("fastctx-sync/checkpoint-state/v1\0{}", step.operation().as_str()).as_bytes());
        let request = AcquireRoot {
            lease: RootLeaseId::from_bytes(digest.bytes()),
            custody: primary.custody.clone(),
            root: root.clone(),
            usage: RootUse::Checkpoint { source: primary.source.clone() },
            bases: Vec::new(),
            expires_at_millis: None,
        };
        let ticket = self
            .supplies
            .checkpoint
            .as_ref()
            .ok_or_else(|| sql::invalid("checkpoint state was not prepared for acquisition"))?;
        let ready = self.writer.recheck(&primary.custody, ticket)?;
        self.writer.acquire(ready, &request)
    }
    pub(crate) fn change_root(&self, step: &SourceStep, change: &RootChange) -> Result<Option<RootLease>> {
        match change {
            RootChange::Acquire(request) => {
                if matches!(request.usage, RootUse::Timeline { .. }) {
                    return match self.writer.acquire_timeline(step.operation(), request)? {
                        RootAcquired::Held { lease } => Ok(Some(*lease)),
                        RootAcquired::Pending { .. } => Err(fastctx_storage::Error::with_kind(
                            fastctx_storage::ErrorKind::Busy,
                            "timeline metadata is not ready",
                        )),
                    };
                }
                let ticket = self.supplies.usage(&request.lease)?;
                let ready = self.writer.recheck(&request.custody, ticket)?;
                self.writer.acquire(ready, request).map(Some)
            }
            RootChange::Release { custody, lease } => {
                root_holds::release(self.db, self.holder, custody, lease)?;
                Ok(None)
            }
        }
    }
}
