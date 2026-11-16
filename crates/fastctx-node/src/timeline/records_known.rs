//! The latest revision of each account record this machine heard of — a
//! write it made, or the hub's hint that one changed — so a read answered
//! from the copy here never answers older than what was already seen
//! (design §15 `switch`). The hub orders these records and every device
//! takes them as replicas a moment after the hub accepts them; a read here
//! that finds its copy behind waits that moment, then asks the hub.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

/// How many records are remembered; past it the oldest heard are forgotten,
/// and a read of one forgotten trusts the copy as it stands.
const REMEMBERED: usize = 65_536;

/// How long a read waits for a copy behind to catch up before asking the hub.
pub(crate) const CATCHING_UP: Duration = Duration::from_millis(1500);

type Key = (String, String);

#[derive(Default)]
struct Known {
    versions: HashMap<Key, (u64, u64)>,
    heard: u64,
}

static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);

/// Revision `version` of the record `kind`/`key` was written or hinted.
pub(crate) fn saw(kind: &str, key: &str, version: u64) {
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    known.heard += 1;
    let heard = known.heard;
    let entry = known.versions.entry((kind.to_string(), key.to_string())).or_insert((0, heard));
    *entry = (entry.0.max(version), heard);
    if known.versions.len() > REMEMBERED {
        let mut ages: Vec<u64> = known.versions.values().map(|(_, heard)| *heard).collect();
        ages.sort_unstable();
        let cut = ages[ages.len() - REMEMBERED / 2];
        known.versions.retain(|_, (_, heard)| *heard >= cut);
    }
}

/// Every record of `kind` whose key begins with `prefix` seen here, with
/// the latest revision of each.
pub(crate) fn under(kind: &str, prefix: &str) -> Vec<(String, u64)> {
    let known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    known
        .versions
        .iter()
        .filter(|((of, key), _)| of == kind && key.starts_with(prefix))
        .map(|((_, key), (version, _))| (key.clone(), *version))
        .collect()
}

/// The latest revision of `kind`/`key` seen here; 0 when none was.
pub(crate) fn at_least(kind: &str, key: &str) -> u64 {
    let known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    known.versions.get(&(kind.to_string(), key.to_string())).map_or(0, |(version, _)| *version)
}
