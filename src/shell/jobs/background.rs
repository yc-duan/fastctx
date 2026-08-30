//! In-memory per-server tracking for background status lines.

use super::model::{JobRecord, JobStatus};
use super::store;
use crate::background_status::{BackgroundEntry, BackgroundState, BackgroundStatus};
use crate::control::paths::ControlPaths;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Clone, Copy, Debug)]
struct TrackedJob {
    started_at: SystemTime,
    started_sort_key: u64,
}

/// Jobs explicitly known to one MCP session; clones share that session state.
#[derive(Clone, Debug, Default)]
pub(super) struct BackgroundTracker {
    jobs: Arc<Mutex<HashMap<String, TrackedJob>>>,
}

impl BackgroundTracker {
    pub(super) fn track_id(&self, job_id: &str, now: SystemTime) {
        let started_sort_key = system_time_nanos(now);
        self.jobs
            .lock()
            .unwrap()
            .entry(job_id.to_string())
            .or_insert(TrackedJob {
                started_at: now,
                started_sort_key,
            });
    }

    pub(super) fn track_record(&self, record: &JobRecord, now: SystemTime) {
        let tracked = tracked_start(record, now);
        self.jobs.lock().unwrap().insert(record.id.clone(), tracked);
    }

    pub(super) fn remove(&self, job_id: &str) {
        self.jobs.lock().unwrap().remove(job_id);
    }

    fn remove_many(&self, job_ids: &[String]) {
        let mut jobs = self.jobs.lock().unwrap();
        for job_id in job_ids {
            jobs.remove(job_id);
        }
    }

    pub(super) fn has_candidates(&self, exclude: Option<&str>) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .keys()
            .any(|job_id| exclude != Some(job_id.as_str()))
    }

    pub(super) fn snapshot(
        &self,
        paths: &ControlPaths,
        exclude: Option<&str>,
        now: SystemTime,
    ) -> Option<BackgroundStatus> {
        self.snapshot_with_probe(exclude, now, |job_id| {
            store::find_record(&paths.jobs_dir, job_id)
        })
    }

    fn snapshot_with_probe(
        &self,
        exclude: Option<&str>,
        now: SystemTime,
        mut probe: impl FnMut(&str) -> Result<Option<JobRecord>, String>,
    ) -> Option<BackgroundStatus> {
        let tracked = self
            .jobs
            .lock()
            .unwrap()
            .iter()
            .filter(|(job_id, _)| exclude != Some(job_id.as_str()))
            .map(|(job_id, tracked)| (job_id.clone(), *tracked))
            .collect::<Vec<_>>();
        if tracked.is_empty() {
            return None;
        }

        let mut entries = Vec::with_capacity(tracked.len());
        let mut missing = Vec::new();
        for (job_id, fallback) in tracked {
            match probe(&job_id) {
                Ok(Some(record)) => {
                    let start = tracked_start(&record, fallback.started_at);
                    self.jobs.lock().unwrap().insert(job_id.clone(), start);
                    let state = match &record.status {
                        JobStatus::Running => BackgroundState::Running,
                        JobStatus::Exited(exit) if exit.was_killed() => BackgroundState::Killed,
                        JobStatus::Exited(exit) => BackgroundState::Exited(exit.exit_code),
                        JobStatus::Interrupted => BackgroundState::Interrupted,
                    };
                    entries.push(BackgroundEntry {
                        job_id,
                        started_at: start.started_at,
                        started_sort_key: start.started_sort_key,
                        state,
                    });
                }
                Ok(None) => missing.push(job_id),
                Err(_) => {}
            }
        }
        if !missing.is_empty() {
            let mut jobs = self.jobs.lock().unwrap();
            for job_id in missing {
                jobs.remove(&job_id);
            }
        }
        let tracker = self.clone();
        BackgroundStatus::render(entries, now)
            .map(|status| status.with_acknowledger(move |job_ids| tracker.remove_many(job_ids)))
    }
}

fn tracked_start(record: &JobRecord, fallback: SystemTime) -> TrackedJob {
    let nanos = if record.meta.started_at_unix_nanos > 0 {
        Some(record.meta.started_at_unix_nanos)
    } else {
        OffsetDateTime::parse(&record.meta.started_at, &Rfc3339)
            .ok()
            .and_then(|value| u64::try_from(value.unix_timestamp_nanos()).ok())
    };
    let Some(started_sort_key) = nanos else {
        return TrackedJob {
            started_at: fallback,
            started_sort_key: system_time_nanos(fallback),
        };
    };
    TrackedJob {
        started_at: UNIX_EPOCH
            .checked_add(Duration::from_nanos(started_sort_key))
            .unwrap_or(fallback),
        started_sort_key,
    }
}

fn system_time_nanos(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
        .unwrap_or(0)
}
