//! Shared values whose registry entry lives exactly as long as their users.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Deref;
use std::sync::{Arc, Mutex, Weak};

type Entries<K, V> = Mutex<HashMap<K, Weak<Entry<K, V>>>>;

pub struct Registry<K: Eq + Hash, V> {
    entries: Arc<Entries<K, V>>,
}

impl<K: Eq + Hash, V> Default for Registry<K, V> {
    fn default() -> Self {
        Self { entries: Arc::default() }
    }
}

pub struct Lease<K: Eq + Hash, V>(Arc<Entry<K, V>>);

struct Entry<K: Eq + Hash, V> {
    key: K,
    value: V,
    registry: Weak<Entries<K, V>>,
}

impl<K: Eq + Hash + Clone, V> Registry<K, V> {
    pub fn get_or_insert_with(&self, key: &K, make: impl FnOnce() -> V) -> Lease<K, V> {
        let mut entries = self.entries.lock().expect("lease registry poisoned");
        if let Some(entry) = entries.get(key).and_then(Weak::upgrade) {
            return Lease(entry);
        }
        let entry = Arc::new(Entry { key: key.clone(), value: make(), registry: Arc::downgrade(&self.entries) });
        entries.insert(key.clone(), Arc::downgrade(&entry));
        Lease(entry)
    }

    pub fn get(&self, key: &K) -> Option<Lease<K, V>> {
        self.entries.lock().expect("lease registry poisoned").get(key).and_then(Weak::upgrade).map(Lease)
    }
}

impl<K: Eq + Hash, V> Clone for Lease<K, V> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<K: Eq + Hash, V> Deref for Lease<K, V> {
    type Target = V;

    fn deref(&self) -> &V {
        &self.0.value
    }
}

impl<K: Eq + Hash, V> Drop for Entry<K, V> {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            let mut entries = registry.lock().expect("lease registry poisoned");
            // A new user can replace an expired weak entry before this destructor locks it.
            if entries.get(&self.key).is_some_and(|entry| std::ptr::eq(entry.as_ptr(), self)) {
                entries.remove(&self.key);
                if entries.is_empty() {
                    *entries = HashMap::new();
                }
            }
        }
    }
}
