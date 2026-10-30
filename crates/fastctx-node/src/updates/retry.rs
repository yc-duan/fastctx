//! Keeps failed acquisitions visible and spaces automatic attempts across restarts.

use fastctx_types::update::{Failed, release};
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Attempt {
    pub version: Option<String>,
    pub delay: fastctx_update::retry::Retry,
    pub failed: Option<Failed>,
}

pub(super) fn save(node: &Node, attempt: &Attempt) {
    if let Err(error) = super::keep(node, "attempt.json", attempt) {
        node.log.line(format_args!("keeping the update attempt: {error}"));
    }
}

pub(super) fn failed(node: &Node, failure: Failed) {
    let mut held = node.updates.held.lock().expect("never poisoned");
    if held.attempt.version.as_deref() != Some(&failure.version) {
        held.attempt.version = Some(failure.version.clone());
        held.attempt.delay = Default::default();
    }
    held.attempt.delay.postpone(jiff::Timestamp::now());
    held.attempt.failed = Some(failure);
    save(node, &held.attempt);
}

pub(super) fn begin(node: &Node, version: &str) -> Option<u32> {
    let mut held = node.updates.held.lock().expect("never poisoned");
    if held.attempt.version.as_deref() == Some(version) {
        if !held.attempt.delay.ready(jiff::Timestamp::now()) {
            return None;
        }
    } else {
        held.attempt.version = Some(version.to_string());
        held.attempt.delay = Default::default();
    }
    // Reserve this attempt while its manifest is resolved, before fetching is set.
    held.attempt.delay.next = jiff::Timestamp::now().checked_add(std::time::Duration::from_secs(3600)).ok();
    save(node, &held.attempt);
    Some(held.attempt.delay.attempts)
}

/// Gives back the attempt reserved for `version`, as while it waits for
/// someone to sign in: a node started again meanwhile begins it at once and
/// waits again, rather than an hour on.
pub(super) fn give_back(node: &Node, version: &str) {
    let mut held = node.updates.held.lock().expect("never poisoned");
    if held.attempt.version.as_deref() == Some(version) && held.attempt.failed.is_none() {
        held.attempt.delay.next = None;
        save(node, &held.attempt);
    }
}

pub(super) fn complete(node: &Node, version: &str) {
    let mut held = node.updates.held.lock().expect("never poisoned");
    let taken = |candidate: &str| release(version).is_some() && release(version) >= release(candidate);
    let mut changed = false;
    if held.attempt.failed.as_ref().is_some_and(|failure| taken(&failure.version)) {
        held.attempt.failed = None;
        changed = true;
    }
    if held.attempt.version.as_deref().is_some_and(taken) {
        held.attempt.version = None;
        held.attempt.delay = Default::default();
        changed = true;
    }
    if changed {
        save(node, &held.attempt);
    }
}
