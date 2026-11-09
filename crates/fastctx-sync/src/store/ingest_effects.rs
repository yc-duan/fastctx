//! Registered, bounded writer plans; ingestion adapters never receive SQL access.
use super::{Error, mutation::Expected, roots::RootChange};
use crate::{
    cursor::Revision,
    identity::{ObjectKey, SourceId},
    registry::{Derived, Kind},
};

pub use crate::registry::source_families::SourceFamily;
#[derive(Clone, Debug)]
pub enum IndexValue {
    Put(Vec<u8>),
    Delete,
}
#[derive(Clone, Debug)]
pub struct SourceIndexChange {
    pub family: SourceFamily,
    pub key: Vec<u8>,
    pub expected: Expected,
    pub value: IndexValue,
}
#[derive(Clone, Debug)]
pub struct SourceIndexRow {
    pub key: Vec<u8>,
    pub revision: Revision,
    pub value: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct SourceIndexRead {
    pub source: SourceId,
    pub family: SourceFamily,
    pub after: Option<Vec<u8>>,
    pub limit: super::PageLimit,
}
impl SourceIndexRead {
    pub fn validate(&self, target: &super::ingest::SourceTarget) -> Result<(), Error> {
        if self.source != target.source
            || !self.family.owner_kinds().contains(&target.custody.kind.registration().id)
            || self.after.as_ref().is_some_and(|key| key.is_empty() || key.len() > 1024)
            || self.limit.items == 0
            || self.limit.items > 256
            || self.limit.bytes == 0
            || self.limit.bytes > fastctx_types::sync::PAGE_BYTES
        {
            return Err(Error::Invalid("source index read exceeds its registered namespace or page bounds".into()));
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct SourceIndexPage {
    pub rows: Vec<SourceIndexRow>,
    pub next: Option<Vec<u8>>,
}
#[derive(Clone)]
pub struct DirtyProjection {
    pub kind: Kind<Derived>,
    pub object: ObjectKey,
}

/// One source's local effects are namespace-bound by the enclosing IngestBatch.
/// The writer assigns dirty wanted/applied sequence from that same commit; a
/// caller cannot invent a local watermark. Source authorizers validate payload
/// semantics separately, using the current trusted permission snapshot.
#[derive(Clone, Default)]
pub struct SourceEffects {
    indexes: Vec<SourceIndexChange>,
    dirty: Vec<DirtyProjection>,
    roots: Vec<RootChange>,
}
impl SourceEffects {
    pub fn new(
        indexes: Vec<SourceIndexChange>,
        dirty: Vec<DirtyProjection>,
        roots: Vec<RootChange>,
    ) -> Result<Self, Error> {
        let effects = Self { indexes, dirty, roots };
        effects.validate_bounds()?;
        Ok(effects)
    }
    pub fn indexes(&self) -> &[SourceIndexChange] {
        &self.indexes
    }
    pub fn dirty(&self) -> &[DirtyProjection] {
        &self.dirty
    }
    pub fn roots(&self) -> &[RootChange] {
        &self.roots
    }
    pub(crate) fn validate_target(&self, target: &super::ingest::SourceTarget) -> Result<(), Error> {
        let object = &target.custody.object;
        if self
            .indexes
            .iter()
            .any(|effect| !effect.family.owner_kinds().contains(&target.custody.kind.registration().id))
            || self.dirty.iter().any(|effect| effect.object.scope != object.scope)
        {
            return Err(Error::Invalid("source effects cross their registered owner".into()));
        }
        for change in &self.roots {
            let (custody, usage) = match change {
                RootChange::Acquire(request) => {
                    request.validate()?;
                    (&request.custody, &request.usage)
                }
                RootChange::Release { custody, lease } => {
                    lease.usage.validate()?;
                    if lease.object != custody.object
                        || lease.kind != custody.kind.registration().id
                        || lease.holding != custody.holding
                        || lease.root.id().domain != object.scope
                    {
                        return Err(Error::Invalid("root release differs from its source custody".into()));
                    }
                    (custody.as_ref(), &lease.usage)
                }
            };
            if custody.object != *object
                || custody.kind.registration().id != target.custody.kind.registration().id
                || custody.holding != target.custody.holding
                || matches!(usage, super::roots::RootUse::Checkpoint { .. })
            {
                return Err(Error::Invalid("source effects cannot replace automatic checkpoint root ownership".into()));
            }
        }
        Ok(())
    }
    pub(crate) fn validate_bounds(&self) -> Result<usize, Error> {
        if self.indexes.len() + self.dirty.len() + self.roots.len() > 256 {
            return Err(Error::Invalid("source writer plan exceeds its effect count".into()));
        }
        let mut bytes = self
            .indexes
            .capacity()
            .checked_mul(size_of::<SourceIndexChange>())
            .and_then(|n| n.checked_add(self.dirty.capacity().checked_mul(512)?))
            .and_then(|n| n.checked_add(self.roots.capacity().checked_mul(4096)?))
            .ok_or(Error::Busy)?;
        let mut keys = std::collections::BTreeSet::new();
        for effect in &self.indexes {
            if effect.key.is_empty() || effect.key.len() > 1024 || !keys.insert((effect.family as u8, &effect.key)) {
                return Err(Error::Invalid("source writer plan has an invalid or repeated key".into()));
            }
            bytes = bytes.checked_add(effect.key.capacity()).ok_or(Error::Busy)?;
            if let IndexValue::Put(body) = &effect.value {
                if body.len() > 64 * 1024 {
                    return Err(Error::Invalid("source index row exceeds its metadata bound".into()));
                }
                bytes = bytes.checked_add(body.capacity()).ok_or(Error::Busy)?;
            }
        }
        if bytes > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Invalid("source writer plan exceeds one metadata page".into()));
        }
        Ok(bytes)
    }
}
