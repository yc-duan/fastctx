//! Immutable dashboard ordering, shared by navigation and rendering.

use crate::shell::jobs::{JobSummary, JobSummaryStatus};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Eq, PartialEq)]
pub(super) enum DashboardRow {
    Source { first_job: usize, total: usize },
    Job(usize),
}

/// Builds source groups once per changed registry snapshot, keeping original registry order.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct JobSnapshot {
    pub(super) jobs: Vec<JobSummary>,
    pub(super) rows: Vec<DashboardRow>,
    pub(super) source_count: usize,
    visible: Vec<usize>,
    focused_rows: Vec<usize>,
}

impl JobSnapshot {
    pub(super) fn new(jobs: Vec<JobSummary>) -> Self {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut indices = HashMap::new();
        for (index, job) in jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| job.status == JobSummaryStatus::Running)
        {
            let group = *indices.entry(job.source.key.as_str()).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[group].push(index);
        }
        let mut rows = Vec::with_capacity(jobs.len() + groups.len());
        let mut visible = Vec::with_capacity(jobs.len());
        let mut focused_rows = Vec::with_capacity(jobs.len());
        for group in groups {
            rows.push(DashboardRow::Source {
                first_job: group[0],
                total: group.len(),
            });
            for index in group {
                visible.push(index);
                focused_rows.push(rows.len());
                rows.push(DashboardRow::Job(index));
            }
        }
        let source_count = jobs
            .iter()
            .map(|job| job.source.key.as_str())
            .collect::<HashSet<_>>()
            .len();
        Self {
            jobs,
            rows,
            source_count,
            visible,
            focused_rows,
        }
    }

    pub(super) fn focused(&self, selected: usize) -> Option<&JobSummary> {
        self.visible.get(selected).map(|index| &self.jobs[*index])
    }

    pub(super) fn position(&self, id: &str) -> Option<usize> {
        self.visible
            .iter()
            .position(|index| self.jobs[*index].id == id)
    }

    pub(super) fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub(super) fn focused_row(&self, selected: usize) -> usize {
        self.focused_rows.get(selected).copied().unwrap_or(0)
    }
}
