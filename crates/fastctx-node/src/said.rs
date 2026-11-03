//! What background work that keeps failing the same way has told already:
//! a failure is told when it starts or changes, not at every try, so the log
//! keeps what is new.

use std::collections::HashMap;
use std::sync::Mutex;

use fastctx_types::ResidentId;

/// Failures kept at once; past it, all are told again once.
const KEPT: usize = 1 << 16;

#[derive(Default)]
pub(crate) struct Said(Mutex<HashMap<(ResidentId, &'static str), String>>);

impl Said {
    /// Whether `failure` of `what` for `id` is to be told: it is new, or not
    /// what was told last. A success (`None`) clears what was told.
    pub(crate) fn changed(&self, id: &ResidentId, what: &'static str, failure: Option<&str>) -> bool {
        let mut said = self.0.lock().expect("never poisoned");
        let key = (id.clone(), what);
        let Some(failure) = failure else {
            said.remove(&key);
            return false;
        };
        if said.get(&key).is_some_and(|told| told == failure) {
            return false;
        }
        if said.len() >= KEPT {
            said.clear();
        }
        said.insert(key, failure.to_string());
        true
    }
}
