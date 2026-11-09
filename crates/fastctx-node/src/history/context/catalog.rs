//! Reader-independent work shared by conversations using one catalog snapshot.

use fastctx_types::history::Catalog;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

pub(crate) struct Prepared {
    pub(super) names: Vec<String>,
    pub(super) topics: BTreeMap<String, String>,
    pub(super) versions: BTreeMap<String, u64>,
    pub(super) listing: String,
}
struct Entry {
    catalog: Weak<Catalog>,
    prepared: Arc<Prepared>,
}
#[derive(Default)]
pub(crate) struct Catalogs(Mutex<BTreeMap<usize, Entry>>);
impl Catalogs {
    pub(crate) fn prepare(&self, catalog: &Arc<Catalog>) -> Arc<Prepared> {
        let mut entries = self.0.lock().expect("history context catalogs");
        let key = Arc::as_ptr(catalog) as usize;
        if let Some(entry) = entries.get(&key)
            && entry.catalog.upgrade().is_some_and(|kept| Arc::ptr_eq(&kept, catalog))
        {
            return entry.prepared.clone();
        }
        entries.retain(|_, entry| entry.catalog.strong_count() != 0);
        let names: Vec<_> = catalog.documents.iter().map(|revision| super::hash(&revision.name)).collect();
        let topics = catalog
            .documents
            .iter()
            .zip(&names)
            .map(|(revision, name)| {
                (name.clone(), super::hash(&format!("{}:{}", revision.retired, revision.description)))
            })
            .collect();
        let versions =
            catalog.documents.iter().zip(&names).map(|(revision, name)| (name.clone(), revision.version)).collect();
        let prepared =
            Arc::new(Prepared { names, topics, versions, listing: fastctx_hands::history::catalog(catalog) });
        entries.insert(key, Entry { catalog: Arc::downgrade(catalog), prepared: prepared.clone() });
        prepared
    }
}
