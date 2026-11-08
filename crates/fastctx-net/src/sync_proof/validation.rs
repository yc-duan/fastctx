use super::open;
use crate::sign::PublicKey;
use fastctx_sync::{
    catalog::SourceBatch,
    proof::{EpochDescriptor, HandoverBasis, ProducerRelease, ProofPurpose},
    registry::{ConflictRule, REGISTRY, ScopePolicy},
    store::Error,
};
use std::collections::BTreeSet;

pub(super) fn device_kind(kind: &str) -> Result<(), Error> {
    let kind = REGISTRY.asset(kind).map_err(Error::Invalid)?;
    if !matches!(kind.registration().scope, ScopePolicy::Device | ScopePolicy::Machine)
        || !matches!(kind.registration().conflict, ConflictRule::Expected | ConflictRule::TextMerge | ConflictRule::Yrs)
    {
        return Err(Error::Denied("a device cannot arbitrate this registered kind".into()));
    }
    Ok(())
}
pub(super) fn source(value: &SourceBatch) -> Result<(), Error> {
    let kind = REGISTRY.asset(&value.kind).map_err(Error::Invalid)?;
    if value.format != 1
        || value.schema != kind.registration().schema
        || kind.registration().conflict != ConflictRule::SingleWriter
        || value.records.is_empty()
        || value.through.0.checked_sub(value.after.0) != Some(value.records.len() as u64)
    {
        return Err(Error::Invalid("invalid signed source range or registered schema".into()));
    }
    let mut operations = BTreeSet::new();
    for (index, record) in value.records.iter().enumerate() {
        if record.sequence.0 != value.after.0 + index as u64 + 1
            || record.kind != value.kind
            || record.schema != value.schema
            || record.payload.domain != value.scope
            || !operations.insert(&record.operation)
        {
            return Err(Error::Invalid("source record differs from its signed range binding".into()));
        }
    }
    Ok(())
}
pub(super) fn epoch(value: &EpochDescriptor) -> Result<(), Error> {
    let kind = REGISTRY.asset(&value.kind).map_err(Error::Invalid)?;
    if kind.registration().conflict != ConflictRule::SingleWriter || kind.registration().schema != value.schema {
        return Err(Error::Invalid("producer epoch names a different registered stream schema".into()));
    }
    if value.predecessor.as_ref().is_some_and(|cursor| {
        cursor.scope != value.scope
            || cursor.stream != value.stream
            || cursor.epoch == value.epoch
            || Some(cursor.committed) != value.closed_through
    }) || value.predecessor.is_none() != value.closed_through.is_none()
    {
        return Err(Error::Invalid("producer epoch has an inconsistent predecessor fence".into()));
    }
    match &value.basis {
        HandoverBasis::Initial { registration } => {
            if value.predecessor.is_some() || registration != &value.operation {
                return Err(Error::Invalid("initial epoch has a predecessor or another registration".into()));
            }
        }
        HandoverBasis::Released { operation, producer_release } => {
            let release: ProducerRelease = open(
                producer_release,
                ProofPurpose::ProducerReleaseV1,
                PublicKey::from_bytes(producer_release.signer.0),
            )?;
            if operation != &value.operation
                || Some(&release.predecessor) != value.predecessor.as_ref()
                || release.successor != value.producer
                || release.epoch != value.epoch
                || release.operation != value.operation
            {
                return Err(Error::Invalid("producer release does not bind this handover".into()));
            }
        }
        HandoverBasis::Restored { operation, .. } => {
            if operation != &value.operation {
                return Err(Error::Invalid("epoch recovery names another operation".into()));
            }
        }
    }
    Ok(())
}
