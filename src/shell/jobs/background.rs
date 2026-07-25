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

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug)]
struct TrackedJob {
    started_at: SystemTime,
    started_sort_key: u64,
}

/// Jobs explicitly known to one server instance; clones share the same session state.
#[derive(Clone, Debug, Default)]
pub(super) struct BackgroundTracker {
    jobs: Arc<Mutex<HashMap<String, TrackedJob>>>,
    #[cfg(test)]
    probes: Arc<AtomicUsize>,
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
            #[cfg(test)]
            self.probes.fetch_add(1, Ordering::Relaxed);
            match probe(&job_id) {
                Ok(Some(record)) => {
                    let start = tracked_start(&record, fallback.started_at);
                    self.jobs.lock().unwrap().insert(job_id.clone(), start);
                    let state = match record.status {
                        JobStatus::Running => BackgroundState::Running,
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
        BackgroundStatus::render(entries, now)
    }

    #[cfg(test)]
    fn probe_count(&self) -> usize {
        self.probes.load(Ordering::Relaxed)
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

#[cfg(test)]
mod tests {
    use super::BackgroundTracker;
    use crate::shell::jobs::model::{
        ExitRecord, JobMeta, JobRecord, JobStatus, OriginSnapshot, ProcessIdentity, TerminationKind,
    };
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn record(id: &str, started: u64, status: JobStatus) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            directory: PathBuf::from(format!("/jobs/{id}")),
            meta: JobMeta {
                schema_version: 3,
                command: "true".to_string(),
                cwd: "/workspace".to_string(),
                login_shell: false,
                encoding: None,
                supervisor: ProcessIdentity {
                    pid: 1,
                    started: "test".to_string(),
                },
                origin: OriginSnapshot {
                    server_pid: 1,
                    server_started: None,
                    parent_pid: None,
                    parent_executable: None,
                    server_cwd: "/workspace".to_string(),
                },
                started_at: "1970-01-01T00:00:00Z".to_string(),
                started_at_unix_nanos: started * 1_000_000_000,
                isolation_warning: None,
            },
            status,
            ended_sort_key: SystemTime::UNIX_EPOCH,
        }
    }

    fn exited(code: i32) -> JobStatus {
        JobStatus::Exited(ExitRecord {
            exit_code: code,
            total_lines: 99,
            had_loss: false,
            ended_at: "1970-01-01T00:00:00Z".to_string(),
            ended_at_unix_nanos: 0,
            termination: TerminationKind::Exited,
            capture_error: None,
        })
    }

    #[test]
    fn empty_or_fully_excluded_tracking_performs_no_registry_probe() {
        let tracker = BackgroundTracker::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        assert!(
            tracker
                .snapshot_with_probe(None, now, |_| panic!("must not probe"))
                .is_none()
        );
        tracker.track_id("j", now);
        assert!(
            tracker
                .snapshot_with_probe(Some("j"), now, |_| panic!("must not probe"))
                .is_none()
        );
        assert_eq!(tracker.probe_count(), 0);
    }

    #[test]
    fn snapshot_sorts_by_persisted_start_and_silently_forgets_evicted_records() {
        let tracker = BackgroundTracker::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        for id in ["late", "early", "gone"] {
            tracker.track_id(id, now);
        }
        let status = tracker
            .snapshot_with_probe(None, now, |id| match id {
                "early" => Ok(Some(record("early", 6_400, JobStatus::Running))),
                "late" => Ok(Some(record("late", 9_940, exited(7)))),
                "gone" => Ok(None),
                _ => unreachable!(),
            })
            .unwrap();
        assert_eq!(
            status.full_line(),
            "(Background: early running 1h0m, late exited 7.)"
        );
        assert_eq!(tracker.probe_count(), 3);
        assert!(!tracker.jobs.lock().unwrap().contains_key("gone"));
    }

    #[test]
    fn separate_trackers_do_not_leak_session_knowledge() {
        let first = BackgroundTracker::default();
        let second = BackgroundTracker::default();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        first.track_id("j", now);
        assert!(first.has_candidates(None));
        assert!(!second.has_candidates(None));
    }
}
