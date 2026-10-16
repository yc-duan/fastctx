//! Oversized entities retain their last representable state without blocking peers.

use fastctx_types::town::{Afloat, Afoot, AfootChange, AfootJob, AfootResident, AfootState};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;

pub type Problems = BTreeMap<String, String>;

/// One earlier snapshot, retained across all frames following `anew`. The wire
/// has no last-frame marker, so the next `anew` replaces this fallback. Accepted
/// replacements and explicit removals retire their earlier values immediately.
#[derive(Default)]
pub struct Previous {
    state: Afoot,
    problems: Problems,
}

pub fn report(previous: &mut Problems, current: Problems, mut log: impl FnMut(String)) {
    for (entity, reason) in &current {
        if previous.get(entity) != Some(reason) {
            log(format!("status {entity} was not updated: {reason}"));
        }
    }
    *previous = current;
}

fn entries<K: Ord + Display, V: Clone + Serialize>(
    next: &mut BTreeMap<K, V>,
    before: Option<&BTreeMap<K, V>>,
    kind: &str,
    problems: &mut Problems,
) {
    next.retain(|id, value| {
        let Err(reason) = super::bounded::fits(value) else { return true };
        problems.insert(format!("{kind} {id}"), reason);
        match before.and_then(|before| before.get(id)) {
            Some(previous) => {
                *value = previous.clone();
                true
            }
            None => false,
        }
    });
}

pub fn state<R: Clone + Serialize, J: Clone + Serialize>(
    next: &mut AfootState<R, J>,
    before: Option<&AfootState<R, J>>,
) -> Problems {
    let mut problems = Problems::new();
    entries(&mut next.residents, before.map(|before| &before.residents), "resident", &mut problems);
    entries(&mut next.jobs, before.map(|before| &before.jobs), "job", &mut problems);
    problems
}

/// Valid snapshots stay shared; only a rejected update needs a corrected copy.
pub fn snapshot<R: Clone + Serialize, J: Clone + Serialize>(
    next: &mut Arc<AfootState<R, J>>,
    before: Option<&AfootState<R, J>>,
) -> Problems {
    if next.residents.values().all(|entry| super::bounded::fits(entry).is_ok())
        && next.jobs.values().all(|entry| super::bounded::fits(entry).is_ok())
    {
        return Problems::new();
    }
    state(Arc::make_mut(next), before)
}

fn changed<K: Ord + Display, V: Afloat<Id = K> + Clone + Serialize>(
    values: &mut Vec<V>,
    before: &BTreeMap<K, V>,
    prior: &mut BTreeMap<K, V>,
    prior_problems: &mut Problems,
    kind: &str,
    problems: &mut Problems,
) {
    values.retain_mut(|value| {
        let id = value.id();
        let key = format!("{kind} {id}");
        problems.remove(&key);
        let Err(reason) = super::bounded::fits(value) else {
            prior.remove(&id);
            prior_problems.remove(&key);
            return true;
        };
        problems.insert(key, reason);
        let previous = before.get(&id).cloned().or_else(|| prior.remove(&id));
        prior.remove(&id);
        match previous {
            Some(previous) => {
                *value = previous;
                true
            }
            None => false,
        }
    });
}

/// Rejected upserts retain accepted values even in later frames of a full snapshot.
pub fn apply(
    state: &mut Afoot,
    prior: &mut Previous,
    mut change: AfootChange<AfootResident, AfootJob>,
    problems: &mut Problems,
    log: impl FnMut(String),
) {
    if change.anew {
        prior.state = std::mem::take(state);
        prior.problems = std::mem::take(problems);
    }
    let mut current = problems.clone();
    for id in &change.residents_gone {
        state.residents.remove(id);
        prior.state.residents.remove(id);
        prior.problems.remove(&format!("resident {id}"));
        current.remove(&format!("resident {id}"));
    }
    for id in &change.jobs_gone {
        state.jobs.remove(id);
        prior.state.jobs.remove(id);
        prior.problems.remove(&format!("job {id}"));
        current.remove(&format!("job {id}"));
    }
    changed(
        &mut change.residents,
        &state.residents,
        &mut prior.state.residents,
        &mut prior.problems,
        "resident",
        &mut current,
    );
    changed(&mut change.jobs, &state.jobs, &mut prior.state.jobs, &mut prior.problems, "job", &mut current);
    for (entity, reason) in &prior.problems {
        problems.entry(entity.clone()).or_insert_with(|| reason.clone());
    }
    report(problems, current, log);
    state.apply(change);
}
