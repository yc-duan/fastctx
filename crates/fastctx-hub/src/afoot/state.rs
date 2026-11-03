//! Activity snapshots by member identity. Shared shards only locate a member;
//! applying its report takes that member's own lock, and fanout owns its cache.

use super::{Afoot, Previous, Problems};
use fastctx_net::sign::PublicKey;
use fastctx_platform::doing::{MemoryMutex, Rank};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const SHARDS: usize = 64;
struct Entry {
    sequence: u64,
    state: Arc<MemoryMutex<Status>>,
}
type Shard = crate::locks::Mutex<HashMap<PublicKey, Entry>>;

#[derive(Default)]
pub(super) struct Status {
    pub told: Arc<Afoot>,
    pub prior: Previous,
    pub rejected: Problems,
}

pub(crate) struct Afoots {
    shards: [Shard; SHARDS],
    pub(super) publishing: Arc<super::publishing::Publishing>,
}

fn shard(key: &PublicKey) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish() as usize % SHARDS
}

impl Default for Afoots {
    fn default() -> Self {
        Self {
            shards: std::array::from_fn(|_| {
                crate::locks::Mutex::new("hub activity index", Rank::HUB_ACTIVITY, HashMap::new())
            }),
            publishing: Arc::default(),
        }
    }
}

impl Afoots {
    pub(super) fn member(&self, key: PublicKey, sequence: u64) -> Arc<MemoryMutex<Status>> {
        let mut shard = self.shards[shard(&key)].lock().expect("activity index");
        let entry = shard.entry(key).or_insert_with(|| Entry {
            sequence,
            state: Arc::new(MemoryMutex::new(
                Rank::HUB_MACHINE_ACTIVITY,
                "member activity",
                key.to_string(),
                Status::default(),
            )),
        });
        entry.sequence = entry.sequence.max(sequence);
        entry.state.clone()
    }

    pub(super) fn get(&self, key: &PublicKey) -> Option<Arc<Afoot>> {
        let member =
            self.shards[shard(key)].lock().expect("activity index").get(key).map(|entry| entry.state.clone())?;
        let snapshot = member.lock().told.clone();
        Some(snapshot)
    }

    pub(super) fn retain(&self, directory: &fastctx_net::directory_index::Indexed<'_>, sequence: u64) {
        for shard in &self.shards {
            let gone: Vec<_> = {
                let held = shard.lock().expect("activity index");
                held.iter()
                    .filter(|(key, entry)| entry.sequence <= sequence && directory.by_key(key).is_none())
                    .map(|(key, _)| *key)
                    .collect()
            };
            for key in gone {
                let mut held = shard.lock().expect("activity index");
                let old = if held.get(&key).is_some_and(|entry| entry.sequence <= sequence) {
                    held.remove(&key)
                } else {
                    None
                };
                drop(held);
                drop(old);
            }
        }
    }

    pub(crate) fn measurements(&self) -> serde_json::Value {
        let shards: Vec<_> = self.shards.iter().map(crate::locks::Mutex::measurements).collect();
        let merged = |name: &str| {
            serde_json::json!({
                "count": shards.iter().map(|s| s[name]["count"].as_u64().unwrap_or(0)).sum::<u64>(),
                "p99_us": shards.iter().filter_map(|s| s[name]["p99_us"].as_u64()).max().unwrap_or(0),
                "max_us": shards.iter().filter_map(|s| s[name]["max_us"].as_u64()).max().unwrap_or(0),
            })
        };
        serde_json::json!({ "name": "hub afoots", "p99_method": "maximum shard upper bound", "holding": merged("holding"), "waiting": merged("waiting") })
    }
}
