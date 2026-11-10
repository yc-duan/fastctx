//! Bounded source-index effects. Reads and planned changes share one namespace;
//! only SourceIngest commits the compare-and-swap revisions.
use fastctx_sync::{
    identity::Digest,
    store::{
        Error,
        ingest::{SourceIngest, SourceTarget},
        ingest_effects::{IndexValue, SourceFamily, SourceIndexChange},
        mutation::Expected,
    },
};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

struct Row {
    family: SourceFamily,
    expected: Expected,
    value: Option<Vec<u8>>,
    changed: bool,
}

pub struct Indexes<'a> {
    source: &'a dyn SourceIngest,
    target: &'a SourceTarget,
    rows: BTreeMap<(u8, Vec<u8>), Row>,
}
impl<'a> Indexes<'a> {
    pub fn new(source: &'a dyn SourceIngest, target: &'a SourceTarget) -> Self {
        Self { source, target, rows: BTreeMap::new() }
    }
    pub async fn get<T: DeserializeOwned>(&mut self, family: SourceFamily, key: &str) -> Result<Option<T>, Error> {
        let id = (family as u8, key.as_bytes().to_vec());
        if !self.rows.contains_key(&id) {
            if self.rows.len() >= 128 {
                return Err(Error::Busy);
            }
            let row = self.source.get_index(self.target.clone(), family, id.1.clone()).await?;
            let (expected, bytes) =
                row.map_or((Expected::Absent, None), |r| (Expected::Revision(r.revision), Some(r.value)));
            self.rows.insert(id.clone(), Row { family, expected, value: bytes, changed: false });
        }
        self.rows
            .get(&id)
            .and_then(|r| r.value.as_deref())
            .map(|bytes| serde_json::from_slice(bytes).map_err(|e| Error::Corrupt(e.to_string())))
            .transpose()
    }
    pub async fn put<T: Serialize>(&mut self, family: SourceFamily, key: &str, value: &T) -> Result<(), Error> {
        let _: Option<serde_json::Value> = self.get(family, key).await?;
        let bytes = fastctx_types::sync::transport_codec::bounded_json(value, 64 * 1024)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let row = self.rows.get_mut(&(family as u8, key.as_bytes().to_vec())).ok_or(Error::Busy)?;
        row.value = Some(bytes.into_boxed_slice().into_vec());
        row.changed = true;
        Ok(())
    }
    pub async fn delete(&mut self, family: SourceFamily, key: &str) -> Result<(), Error> {
        let _: Option<serde_json::Value> = self.get(family, key).await?;
        let row = self.rows.get_mut(&(family as u8, key.as_bytes().to_vec())).ok_or(Error::Busy)?;
        row.value = None;
        row.changed = true;
        Ok(())
    }
    pub fn effects(self) -> Vec<SourceIndexChange> {
        self.rows
            .into_iter()
            .filter_map(|((_, key), Row { family, expected, value, changed })| {
                changed.then(|| SourceIndexChange {
                    family,
                    key,
                    expected,
                    value: value.map_or(IndexValue::Delete, IndexValue::Put),
                })
            })
            .collect()
    }
}
pub fn key(domain: &str, native: &str) -> String {
    format!("claude/{domain}/{}", Digest::of(native.as_bytes()).as_str())
}
