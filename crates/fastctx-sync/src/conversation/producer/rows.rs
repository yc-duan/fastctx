use super::{api::*, keys};
use crate::{
    cursor::Revision,
    identity::ObjectId,
    store::{
        PageLimit,
        ingest::{SourceIngest, SourceTarget},
        ingest_effects::{IndexValue, SourceFamily, SourceIndexChange, SourceIndexRead, SourceIndexRow},
        mutation::Expected,
    },
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Row {
    original: Option<(Revision, Vec<u8>)>,
    value: Option<Vec<u8>>,
}
#[derive(Clone)]
pub(super) struct Rows {
    prefix: Vec<u8>,
    rows: BTreeMap<Vec<u8>, Row>,
    max_bytes: usize,
}
impl Rows {
    pub fn new(conversation: &ObjectId, max_bytes: usize) -> Self {
        Self { prefix: keys::namespace(conversation), rows: BTreeMap::new(), max_bytes }
    }
    pub fn key(&self, key: &str) -> Vec<u8> {
        [self.prefix.as_slice(), key.as_bytes()].concat()
    }
    pub fn suffix<'a>(&self, key: &'a [u8]) -> Result<&'a str> {
        let key = key.strip_prefix(self.prefix.as_slice()).ok_or_else(|| MergeError::CorruptState {
            key: key.to_vec(),
            reason: "key crosses reducer namespace".into(),
        })?;
        std::str::from_utf8(key).map_err(|e| MergeError::CorruptState { key: key.to_vec(), reason: e.to_string() })
    }
    pub async fn get<T: DeserializeOwned>(
        &mut self,
        source: &dyn SourceIngest,
        target: &SourceTarget,
        key: &str,
    ) -> Result<Option<T>> {
        let key = self.key(key);
        self.load(source, target, &key).await?;
        self.rows
            .get(&key)
            .and_then(|r| r.value.as_ref())
            .map(|v| {
                serde_json::from_slice(v)
                    .map_err(|e| MergeError::CorruptState { key: key.clone(), reason: e.to_string() })
            })
            .transpose()
    }
    async fn load(&mut self, source: &dyn SourceIngest, target: &SourceTarget, key: &[u8]) -> Result<()> {
        if self.rows.contains_key(key) {
            return Ok(());
        }
        let row = source.get_index(target.clone(), SourceFamily::Reducer, key.to_vec()).await?;
        self.remember(key.to_vec(), row)?;
        Ok(())
    }
    fn remember(&mut self, key: Vec<u8>, row: Option<SourceIndexRow>) -> Result<()> {
        if let Some(row) = &row
            && (row.key != key || row.value.len() > 65536 || row.revision.0 == 0)
        {
            return Err(MergeError::CorruptState { key, reason: "invalid source index row".into() });
        }
        let original = row.map(|r| (r.revision, r.value));
        let value = original.as_ref().map(|(_, v)| v.clone());
        self.rows.insert(key, Row { original, value });
        self.check_memory()
    }
    pub async fn put<T: Serialize>(
        &mut self,
        source: &dyn SourceIngest,
        target: &SourceTarget,
        key: &str,
        value: &T,
    ) -> Result<bool> {
        let bytes = fastctx_types::sync::transport_codec::bounded_json(value, 65536)
            .map_err(|_| MergeError::Limit(BudgetKind::EffectBytes))?;
        self.set(source, target, key, Some(bytes.into_boxed_slice().into_vec())).await
    }
    pub async fn delete(&mut self, source: &dyn SourceIngest, target: &SourceTarget, key: &str) -> Result<bool> {
        self.set(source, target, key, None).await
    }
    async fn set(
        &mut self,
        source: &dyn SourceIngest,
        target: &SourceTarget,
        key: &str,
        value: Option<Vec<u8>>,
    ) -> Result<bool> {
        let key = self.key(key);
        if key.len() > 1024 {
            return Err(MergeError::InvalidInput("reducer key exceeds 1024 bytes".into()));
        }
        self.load(source, target, &key).await?;
        let row = self.rows.get_mut(&key).ok_or_else(|| MergeError::InvalidInput("unloaded index row".into()))?;
        let changed = row.value != value;
        row.value = value;
        self.check_memory()?;
        Ok(changed)
    }
    /// Returns only one visible row. Deleted persistent rows are skipped without
    /// letting an earlier overlay insertion escape the merged key order.
    pub async fn first(
        &mut self,
        source: &dyn SourceIngest,
        target: &SourceTarget,
        prefix: &str,
        after: Option<&[u8]>,
    ) -> Result<Option<SourceIndexRow>> {
        let prefix = self.key(prefix);
        let mut cursor = after.map_or_else(|| prefix.clone(), <[u8]>::to_vec);
        loop {
            let local = self
                .rows
                .range((std::ops::Bound::Excluded(cursor.clone()), std::ops::Bound::Unbounded))
                .find(|(key, r)| key.starts_with(&prefix) && r.value.is_some())
                .map(|(k, _)| k.clone());
            let page = source
                .read_index(
                    target.clone(),
                    SourceIndexRead {
                        source: target.source.clone(),
                        family: SourceFamily::Reducer,
                        after: Some(cursor.clone()),
                        limit: PageLimit { items: 1, bytes: fastctx_types::sync::PAGE_BYTES },
                    },
                )
                .await?;
            let persisted = page.rows.into_iter().next().filter(|r| r.key.starts_with(&prefix));
            let key = match (&local, &persisted) {
                (Some(k), Some(r)) => k.min(&r.key).clone(),
                (Some(k), None) => k.clone(),
                (None, Some(r)) => r.key.clone(),
                (None, None) => return Ok(None),
            };
            if !self.rows.contains_key(&key) {
                self.remember(key.clone(), persisted.filter(|r| r.key == key))?;
            }
            let row = self.rows.get(&key).ok_or_else(|| MergeError::InvalidInput("unloaded page row".into()))?;
            if let Some(value) = &row.value {
                return Ok(Some(SourceIndexRow {
                    key,
                    revision: row.original.as_ref().map_or(Revision(0), |(r, _)| *r),
                    value: value.clone(),
                }));
            }
            cursor = key;
        }
    }
    pub async fn exists(&self, source: &dyn SourceIngest, target: &SourceTarget, prefix: &str) -> Result<bool> {
        let prefix = self.key(prefix);
        let mut cursor = prefix.clone();
        loop {
            let local = self
                .rows
                .range((std::ops::Bound::Excluded(cursor.clone()), std::ops::Bound::Unbounded))
                .find(|(k, r)| k.starts_with(&prefix) && r.value.is_some())
                .map(|(k, _)| k.clone());
            let page = source
                .read_index(
                    target.clone(),
                    SourceIndexRead {
                        source: target.source.clone(),
                        family: SourceFamily::Reducer,
                        after: Some(cursor.clone()),
                        limit: PageLimit { items: 1, bytes: fastctx_types::sync::PAGE_BYTES },
                    },
                )
                .await?;
            let persistent = page.rows.into_iter().next().filter(|r| r.key.starts_with(&prefix));
            let key = match (local, persistent) {
                (Some(local), Some(row)) => local.min(row.key),
                (Some(local), None) => local,
                (None, Some(row)) => row.key,
                (None, None) => return Ok(false),
            };
            if self.rows.get(&key).is_none_or(|r| r.value.is_some()) {
                return Ok(true);
            }
            cursor = key;
        }
    }
    pub fn changes(&self) -> Vec<SourceIndexChange> {
        self.rows
            .iter()
            .filter_map(|(key, row)| {
                if row.original.as_ref().map(|(_, v)| v) == row.value.as_ref() {
                    return None;
                }
                Some(SourceIndexChange {
                    family: SourceFamily::Reducer,
                    key: key.clone().into_boxed_slice().into_vec(),
                    expected: row.original.as_ref().map_or(Expected::Absent, |(r, _)| Expected::Revision(*r)),
                    value: row
                        .value
                        .as_ref()
                        .map_or(IndexValue::Delete, |v| IndexValue::Put(v.clone().into_boxed_slice().into_vec())),
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .into_vec()
    }
    pub fn bytes(&self) -> usize {
        self.rows
            .iter()
            .map(|(k, r)| {
                k.capacity()
                    + 128
                    + r.original.as_ref().map_or(0, |(_, v)| v.capacity())
                    + r.value.as_ref().map_or(0, Vec::capacity)
            })
            .sum()
    }
    fn check_memory(&self) -> Result<()> {
        if self.bytes().saturating_mul(2) > self.max_bytes {
            Err(MergeError::Limit(BudgetKind::WorkBytes))
        } else {
            Ok(())
        }
    }
}
