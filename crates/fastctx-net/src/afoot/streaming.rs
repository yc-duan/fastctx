//! A diff of shared snapshots, materialized one wire frame at a time.

use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

use fastctx_types::town::{Afloat, AfootChange, AfootState};
use fastctx_types::{JobId, ResidentId};
use serde::Serialize;

pub struct Frames<R, J> {
    before: Option<Arc<AfootState<R, J>>>,
    after: Arc<AfootState<R, J>>,
    phase: u8,
    resident: Option<ResidentId>,
    job: Option<JobId>,
    pending: Option<Box<Item<R, J>>>,
    anew: bool,
    done: bool,
}

enum Item<R, J> {
    Resident(R),
    ResidentGone(ResidentId),
    Job(J),
    JobGone(JobId),
}

fn next<'a, K: Ord, V>(map: &'a BTreeMap<K, V>, after: Option<&K>) -> Option<(&'a K, &'a V)> {
    map.range((after.map_or(Unbounded, Excluded), Unbounded)).next()
}

impl<R, J> Frames<R, J> {
    pub fn new(before: Option<Arc<AfootState<R, J>>>, after: Arc<AfootState<R, J>>) -> Self {
        let anew = before.is_none();
        Self { before, after, phase: 0, resident: None, job: None, pending: None, anew, done: false }
    }

    /// Replaces an update that has not started draining; its original baseline remains.
    pub fn replace(&mut self, after: Arc<AfootState<R, J>>) {
        assert!(self.phase == 0 && self.resident.is_none() && self.job.is_none() && self.pending.is_none());
        self.after = after;
    }

    pub fn done(&self) -> bool {
        self.done
    }
}

impl<R, J> Frames<R, J>
where
    R: Afloat<Id = ResidentId> + Clone + PartialEq,
    J: Afloat<Id = JobId> + Clone + PartialEq,
{
    fn item(&mut self) -> Option<Item<R, J>> {
        loop {
            match self.phase {
                0 => {
                    if let Some((id, _)) =
                        self.before.as_ref().and_then(|before| next(&before.residents, self.resident.as_ref()))
                    {
                        self.resident = Some(id.clone());
                        if !self.after.residents.contains_key(id) {
                            return Some(Item::ResidentGone(id.clone()));
                        }
                        continue;
                    }
                    self.resident = None;
                    self.phase = 1;
                }
                1 => {
                    if let Some((id, value)) = next(&self.after.residents, self.resident.as_ref()) {
                        self.resident = Some(id.clone());
                        if self.before.as_ref().and_then(|before| before.residents.get(id)) != Some(value) {
                            return Some(Item::Resident(value.clone()));
                        }
                        continue;
                    }
                    self.phase = 2;
                }
                2 => {
                    if let Some((id, _)) = self.before.as_ref().and_then(|before| next(&before.jobs, self.job.as_ref()))
                    {
                        self.job = Some(id.clone());
                        if !self.after.jobs.contains_key(id) {
                            return Some(Item::JobGone(id.clone()));
                        }
                        continue;
                    }
                    self.job = None;
                    self.phase = 3;
                }
                3 => {
                    if let Some((id, value)) = next(&self.after.jobs, self.job.as_ref()) {
                        self.job = Some(id.clone());
                        if self.before.as_ref().and_then(|before| before.jobs.get(id)) != Some(value) {
                            return Some(Item::Job(value.clone()));
                        }
                        continue;
                    }
                    self.phase = 4;
                }
                _ => return None,
            }
        }
    }
}

impl<R, J> Iterator for Frames<R, J>
where
    R: Afloat<Id = ResidentId> + Clone + PartialEq + Serialize,
    J: Afloat<Id = JobId> + Clone + PartialEq + Serialize,
{
    type Item = Result<AfootChange<R, J>, String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut change = AfootChange { anew: std::mem::take(&mut self.anew), ..AfootChange::default() };
        let mut used = 128;
        loop {
            let Some(item) = self.pending.take().map(|item| *item).or_else(|| self.item()) else {
                self.done = true;
                return Some(Ok(change));
            };
            let bytes = match &item {
                Item::Resident(value) => serde_json::to_vec(value),
                Item::ResidentGone(value) => serde_json::to_vec(value),
                Item::Job(value) => serde_json::to_vec(value),
                Item::JobGone(value) => serde_json::to_vec(value),
            }
            .expect("status values serialize")
            .len()
                + 1;
            if bytes + 128 > super::FRAME_ROOM {
                self.done = true;
                return Some(Err(format!("a status item needs {bytes} bytes and cannot fit in a control message")));
            }
            if used + bytes > super::FRAME_ROOM {
                self.pending = Some(Box::new(item));
                return Some(Ok(change));
            }
            used += bytes;
            match item {
                Item::Resident(value) => change.residents.push(value),
                Item::ResidentGone(value) => change.residents_gone.push(value),
                Item::Job(value) => change.jobs.push(value),
                Item::JobGone(value) => change.jobs_gone.push(value),
            }
        }
    }
}
