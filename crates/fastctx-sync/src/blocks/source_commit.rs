//! The content side of a source commit. Inside the source writer's transaction it
//! adopts what a step supplied, swaps the checkpoint's state hold and closes the step.
use super::{access, manifest::Reference, pages, pins, root_holds, rows, steps};
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

/// Rechecks the holder's current custody of a destination in this transaction.
pub(crate) fn authorize(db: &Connection, holder: &Holder, policy: &dyn AccessPolicy, custody: &Custody) -> Result<()> {
    pins::authorize(db, holder, policy, custody)
}

/// Publishes `root` as the destination object's content. The root must have been
/// supplied by this step or already belong to the object, and its closure must be
/// complete; access expansion then carries the grant through the closure.
pub(crate) fn adopt(db: &Connection, step: &SourceStep, custody: &Custody, root: &ContentRef) -> Result<()> {
    if !pins::ready_for(db, step.operation(), custody, root)? && !access::has_grant(db, custody, root)? {
        return Err(sql::invalid("a source record names content its step did not supply"));
    }
    if !rows::complete(db, root)? {
        return Err(sql::invalid("a source record names content whose closure is incomplete"));
    }
    let kind = custody.kind.registration().id;
    access::grant_root(db, &custody.object, kind, root, &custody.holding)?;
    // A descriptor's own references are readable as soon as it is committed;
    // deeper levels expand through the content worker as usual.
    if let ContentRef::Manifest { root: page } = root {
        let references: Vec<_> = pages::references(db, &page.0.domain, &page.0.digest)?
            .into_iter()
            .map(|child| match child {
                ContentRef::Single { block, decoded_bytes } => Reference::Block { id: block, decoded_bytes },
                ContentRef::Manifest { root } => Reference::Manifest { root },
            })
            .collect();
        access::expand(db, &custody.object, kind, page, &references, &custody.holding)?;
    }
    Ok(())
}

/// Holds the new checkpoint's private parser state while that checkpoint is
/// current. The hold grants no read, and its identity is fixed by the step.
pub(crate) fn hold_checkpoint(db: &Connection, step: &SourceStep, root: &ContentRef, now: i64) -> Result<RootLease> {
    let primary = step.primary();
    let digest = Digest::of(format!("fastctx-sync/checkpoint-state/v1\0{}", step.operation().as_str()).as_bytes());
    let request = AcquireRoot {
        lease: RootLeaseId::from_bytes(digest.bytes()),
        custody: primary.custody.clone(),
        root: root.clone(),
        usage: RootUse::Checkpoint { source: primary.source.clone() },
        expires_at_millis: None,
    };
    match root_holds::acquire(db, step.operation(), &request, now)? {
        RootAcquired::Held { lease } => Ok(*lease),
        RootAcquired::Pending { .. } => Err(sql::invalid("checkpoint state must be a complete supplied closure")),
    }
}

/// Releases a hold this writer owns, such as the superseded checkpoint's state.
pub(crate) fn release(db: &Connection, holder: &Holder, custody: &Custody, lease: &RootLease) -> Result<()> {
    root_holds::release(db, holder, custody, lease)
}

/// Applies one root change of a source effect; returns the use it acquired.
pub(crate) fn change_root(
    db: &Connection,
    holder: &Holder,
    step: &SourceStep,
    change: &RootChange,
    now: i64,
) -> Result<Option<RootLease>> {
    match change {
        RootChange::Acquire(request) => match root_holds::acquire(db, step.operation(), request, now)? {
            RootAcquired::Held { lease } => Ok(Some(*lease)),
            RootAcquired::Pending { .. } => Err(sql::invalid("a source root use needs a complete closure")),
        },
        RootChange::Release { custody, lease } => {
            root_holds::release(db, holder, custody, lease)?;
            Ok(None)
        }
    }
}
