//! The health gate of a version going out a batch at a time
//! (`specs/world-engine.md`, 发布与推广): while a rollout goes on, the machines
//! that took the version are watched beside the others. When more of them
//! had trouble since they took it than of the others lately, the rollout
//! stops where it is, and the super administrator hears of it.

use std::collections::HashMap;
use std::sync::Weak;
use std::time::Duration;

use fastctx_net::sign::PublicKey;
use fastctx_types::trouble::TroubleKind;
use fastctx_types::update::Stop;
use jiff::{SignedDuration, Timestamp};

use crate::Hub;

/// How often the machines on the version are looked at.
const EVERY: Duration = Duration::from_secs(60);
/// How far back the machines not on it are looked at.
const LATELY: SignedDuration = SignedDuration::from_hours(6);
/// The troubles that count against a version; earlier records failing to
/// move, or found unreadable, are the first sign of a version that cannot
/// take its machines' records.
const COUNTED: [TroubleKind; 5] = [
    TroubleKind::Stuck,
    TroubleKind::Drops,
    TroubleKind::Silent,
    TroubleKind::Migration,
    TroubleKind::Damaged,
];

/// The machine on the version, and its trouble, when more of the machines
/// on it had trouble since they took it than of the rest lately: of the
/// rest, only machines that said what they run count, as only they are
/// counted among them. `took` says when each machine on the version took it,
/// `running` what each machine said it runs; `troubles` are each machine's
/// troubles lately, by key, kind and when.
fn fared_worse<K: Copy + Eq + std::hash::Hash, V>(
    took: &HashMap<K, Timestamp>,
    reported: usize,
    running: &HashMap<K, V>,
    troubles: Vec<(K, TroubleKind, Timestamp)>,
) -> Option<(K, TroubleKind)> {
    // The first trouble of each machine on the version since it took it.
    let mut on: HashMap<K, (TroubleKind, Timestamp)> = HashMap::new();
    let mut others = std::collections::HashSet::new();
    for (key, kind, at) in troubles {
        match took.get(&key) {
            Some(since) if at >= *since => {
                on.entry(key).or_insert((kind, at));
            }
            Some(_) => {}
            None if running.contains_key(&key) => {
                others.insert(key);
            }
            None => {}
        }
    }
    let rest = reported.saturating_sub(took.len());
    // Each as a share of its own, cross-multiplied so none is divided by
    // nothing: with no rest to compare with, nothing fared worse.
    if on.is_empty() || on.len() * rest <= others.len() * took.len() {
        return None;
    }
    on.into_iter().min_by_key(|(_, (_, at))| *at).map(|(key, (kind, _))| (key, kind))
}

/// Watches every rollout while the hub runs.
pub(crate) async fn watch(hub: Weak<Hub>) {
    // When each machine took the version going out: the look before it was
    // first seen running it, as a machine tells of trouble from its first
    // moments on a version (records it cannot move, found as it starts),
    // before the next look. Kept while the rollout is paused, to go on
    // from; a hub started again begins anew.
    let mut took: HashMap<PublicKey, Timestamp> = HashMap::new();
    let mut watched: Option<String> = None;
    let mut looked: Option<Timestamp> = None;
    loop {
        tokio::time::sleep(EVERY).await;
        let Some(hub) = hub.upgrade() else { return };
        let Some(rollout) = hub.store().hub.updates.rollout.clone() else {
            took.clear();
            watched = None;
            looked = None;
            continue;
        };
        if watched.as_deref() != Some(rollout.version.as_str()) {
            took.clear();
            watched = Some(rollout.version.clone());
        }
        let now = Timestamp::now();
        let since = looked.replace(now).unwrap_or(now);
        let running = super::running_kernels(&hub);
        took.retain(|key, _| running.get(key).is_some_and(|kernel| *kernel == rollout.version));
        for (key, kernel) in &running {
            if *kernel == rollout.version {
                took.entry(*key).or_insert(since);
            }
        }
        if rollout.paused {
            continue;
        }
        if took.is_empty() {
            continue;
        }
        let lately = now.checked_sub(LATELY).unwrap_or(now);
        let troubles = hub.troubles.of_kinds_since(&COUNTED, lately);
        let Some((key, kind)) = fared_worse(&took, running.len(), &running, troubles) else { continue };
        let Some(machine) = hub.store().indexed().by_key(&key).map(|member| member.name.clone()) else { continue };
        let version = rollout.version.clone();
        let stop = Stop { at: now, machine: machine.clone(), kind };
        let stopped = hub
            .blocking(move |hub| -> std::io::Result<bool> {
                let mut store = hub.edit_store();
                let mut updates = store.hub.updates.clone();
                let Some(current) =
                    updates.rollout.as_mut().filter(|current| current.version == version && !current.paused)
                else {
                    return Ok(false);
                };
                current.paused = true;
                current.stopped = Some(stop);
                store.hub.updates = updates;
                store.save_hub()?;
                Ok(true)
            })
            .await;
        match stopped {
            Ok(true) => {
                hub.log.line(format_args!(
                    "stopped rolling out {}: {machine} had {kind:?} trouble after it took it",
                    rollout.version
                ));
                super::tell(&hub, None);
                let detail = crate::troubles::Detail {
                    version: Some(rollout.version.clone()),
                    ..crate::troubles::Detail::default()
                };
                hub.troubles.raise(&hub, None, TroubleKind::Rollout, detail);
            }
            Ok(false) => {}
            Err(error) => hub.log.line(format_args!("stopping the rollout of {}: {error}", rollout.version)),
        }
    }
}
