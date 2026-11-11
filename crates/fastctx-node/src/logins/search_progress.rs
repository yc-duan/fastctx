//! The in-memory progress readers see while a folder search is running.
use super::folders::{Searching, below};
use crate::Node;
use fastctx_hosts::inventory::walk;
use fastctx_types::logins::{FolderSearch, MachineLogins, Stage};
use jiff::Timestamp;
use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
};

/// Overlays bounded, in-memory search progress without consulting the disk.
pub(super) fn overlay(node: &Node, value: &mut MachineLogins) {
    let known: HashSet<_> = [&value.codex, &value.claude]
        .into_iter()
        .chain(value.others.values())
        .flat_map(|side| side.listed.iter().chain(&side.candidates).map(|login| walk::key(&login.dir)))
        .collect();
    let keys: Vec<_> = value.folders.iter().map(|folder| walk::key(&folder.dir)).collect();
    let mut remaining = 4096;
    for folder in &mut value.folders {
        let key = walk::key(&folder.dir);
        let Some(running) = node.logins.searches.get(&key) else { continue };
        let found = running.found.lock().expect("never poisoned");
        let overflow = found.len() > remaining;
        let mut shown: Vec<_> = found.iter().take(remaining).cloned().collect();
        remaining -= shown.len();
        drop(found);
        shown.retain(|one| {
            let path = walk::key(&folder.dir.join(&one.path));
            !known.contains(&path) && !keys.iter().any(|inner| below(inner, &key) && below(&path, inner))
        });
        shown.sort_by(|a, b| a.path.cmp(&b.path));
        folder.search = Some(FolderSearch {
            stage: if overflow {
                Stage::Failed { at: Timestamp::now(), reason: "the search display exceeds 4096 results".into() }
            } else if running.stop.load(Ordering::Relaxed) {
                Stage::Stopped { at: Timestamp::now() }
            } else {
                Stage::Running
            },
            directories: running.counters.directories.load(Ordering::Relaxed),
            unreadable: running.counters.unreadable.load(Ordering::Relaxed),
            found: shown,
        });
    }
}

/// Announces changing counters only while this particular search is running.
pub(super) fn follow(node: &Arc<Node>, key: &str, searching: &Arc<Searching>) {
    let (weak, key, searching) = (Arc::downgrade(node), key.to_owned(), Arc::downgrade(searching));
    fastctx_platform::task::spawn(async move {
        let mut last = None;
        loop {
            let (Some(node), Some(searching)) = (weak.upgrade(), searching.upgrade()) else { return };
            if !node.logins.searches.current(&key, &searching) {
                return;
            }
            let now = (
                searching.counters.directories.load(Ordering::Relaxed),
                searching.counters.unreadable.load(Ordering::Relaxed),
                searching.found.lock().expect("never poisoned").len(),
                searching.stop.load(Ordering::Relaxed),
            );
            if last != Some(now) {
                node.topics.changed(fastctx_types::topics::LOGINS);
                last = Some(now);
            }
            drop(node);
            drop(searching);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
}
