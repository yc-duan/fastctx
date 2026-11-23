//! A conversation's timeline failing, told in the node's log once for each
//! failure that follows another or a look that brought it: a job that fails
//! is tried again at every change, and otherwise its failure is kept only
//! with the job (`fastctx-jobs`), where nothing reads it.

use fastctx_types::ResidentId;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// The last failure told of each conversation's timeline.
static FAILED: LazyLock<Mutex<HashMap<ResidentId, String>>> = LazyLock::new(Mutex::default);

/// The most conversations whose last failure is held: past them all are let
/// go, those of conversations long quiet among them.
const HELD_MOST: usize = 4096;

fn held() -> std::sync::MutexGuard<'static, HashMap<ResidentId, String>> {
    FAILED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `resident`'s timeline failed with `message`: told unless it is the last
/// failure told of it.
pub(super) fn failed(node: &crate::Node, resident: &ResidentId, message: &str) {
    let mut failed = held();
    if failed.get(resident).is_some_and(|told| told == message) {
        return;
    }
    if failed.len() >= HELD_MOST {
        failed.clear();
    }
    failed.insert(resident.clone(), message.to_string());
    drop(failed);
    node.log.line(format_args!("{resident}'s asset timeline failed: {message}"));
}

/// `resident`'s timeline was brought: a failure after it is told again.
pub(super) fn brought(resident: &ResidentId) {
    held().remove(resident);
}
