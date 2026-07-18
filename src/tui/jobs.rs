//! Aggregated running-job state, output navigation, and bounded viewports.

use crate::shell::jobs::{JobSourceSummary, JobSummary, JobSummaryStatus, JobTail};
use ratatui::text::Line;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

/// Escapes terminal controls without allocating for ordinary output lines.
pub(crate) fn display_output_line(value: &str) -> Cow<'_, str> {
    if value.chars().all(|character| !character.is_control()) {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .chars()
            .flat_map(|character| {
                if character.is_control() {
                    character.escape_default().collect::<Vec<_>>()
                } else {
                    vec![character]
                }
            })
            .collect(),
    )
}

/// Complete data model for the current user's cross-session job dashboard.
#[derive(Clone, Debug)]
pub(crate) enum JobsState {
    Loading,
    Ready(Arc<[JobSummary]>),
    Empty,
    PermissionDenied(String),
    Error(String),
}

impl JobsState {
    pub(crate) fn ready(jobs: Vec<JobSummary>) -> Self {
        Self::Ready(jobs.into())
    }

    pub(crate) fn jobs(&self) -> &[JobSummary] {
        match self {
            Self::Ready(jobs) => jobs.as_ref(),
            Self::Loading | Self::Empty | Self::PermissionDenied(_) | Self::Error(_) => &[],
        }
    }
}

/// One source session and its running jobs, preserving registry order inside each group.
#[derive(Debug)]
pub(crate) struct JobGroup<'a> {
    pub(crate) source: &'a JobSourceSummary,
    pub(crate) jobs: Vec<&'a JobSummary>,
    pub(crate) total: usize,
}

/// Groups running jobs by immutable source identity; terminal records never enter the dashboard.
pub(crate) fn grouped_jobs(jobs: &[JobSummary]) -> Vec<JobGroup<'_>> {
    let mut groups = Vec::<JobGroup<'_>>::new();
    let mut group_indices = HashMap::<&str, usize>::new();
    for job in jobs
        .iter()
        .filter(|job| job.status == JobSummaryStatus::Running)
    {
        let group_index = match group_indices.get(job.source.key.as_str()) {
            Some(index) => *index,
            None => {
                let index = groups.len();
                group_indices.insert(job.source.key.as_str(), index);
                groups.push(JobGroup {
                    source: &job.source,
                    jobs: Vec::new(),
                    total: 0,
                });
                index
            }
        };
        let group = &mut groups[group_index];
        group.total = group.total.saturating_add(1);
        group.jobs.push(job);
    }
    groups
}

pub(crate) fn visible_jobs(jobs: &[JobSummary]) -> Vec<&JobSummary> {
    grouped_jobs(jobs)
        .into_iter()
        .flat_map(|group| group.jobs)
        .collect()
}

pub(crate) fn visible_job_count(jobs: &[JobSummary]) -> usize {
    visible_jobs(jobs).len()
}

pub(crate) fn source_count(jobs: &[JobSummary]) -> usize {
    jobs.iter()
        .map(|job| job.source.key.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Read-only output detail and viewport for the focused job.
#[derive(Clone, Debug)]
pub(crate) struct JobsDetail {
    pub(crate) job_id: Option<String>,
    pub(crate) tail: JobTail,
    pub(crate) error: Option<String>,
    pub(crate) horizontal_offset: usize,
    pub(crate) lines_below: usize,
    pub(crate) follow_tail: bool,
}

impl Default for JobsDetail {
    fn default() -> Self {
        Self {
            job_id: None,
            tail: JobTail::default(),
            error: None,
            horizontal_offset: 0,
            lines_below: 0,
            follow_tail: true,
        }
    }
}

impl JobsDetail {
    pub(crate) fn move_horizontal(&mut self, forward: bool) {
        const STEP: usize = 8;
        if forward {
            let max_offset = self
                .tail
                .lines
                .iter()
                .map(|line| {
                    Line::from(display_output_line(line))
                        .width()
                        .saturating_sub(1)
                })
                .max()
                .unwrap_or(0);
            self.horizontal_offset = self.horizontal_offset.saturating_add(STEP).min(max_offset);
        } else {
            self.horizontal_offset = self.horizontal_offset.saturating_sub(STEP);
        }
    }

    pub(crate) fn page_output(&mut self, toward_tail: bool) {
        const PAGE: usize = 8;
        if toward_tail {
            self.lines_below = self.lines_below.saturating_sub(PAGE);
            if self.lines_below == 0 {
                self.follow_tail = true;
            }
        } else {
            self.follow_tail = false;
            self.lines_below = self.lines_below.saturating_add(PAGE);
        }
    }

    pub(crate) fn jump_to_output_edge(&mut self, tail: bool) {
        if tail {
            self.lines_below = 0;
            self.follow_tail = true;
        } else {
            self.follow_tail = false;
            self.lines_below = self.tail.lines.len();
        }
    }

    pub(crate) fn toggle_follow(&mut self) {
        self.follow_tail = !self.follow_tail;
        if self.follow_tail {
            self.lines_below = 0;
        }
    }

    pub(crate) fn preserve_view_after_append(&mut self, appended: usize) {
        if !self.follow_tail {
            self.lines_below = self.lines_below.saturating_add(appended);
        }
    }
}

/// Bounded dashboard viewport whose offset is anchored to rendered rows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct JobsViewport {
    offset: usize,
}

/// Content window and edge markers for one render.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct JobsViewportWindow {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) show_above: bool,
    pub(crate) show_below: bool,
}

impl JobsViewport {
    /// Keeps the focused job row visible while reserving marker rows when possible.
    pub(crate) fn window(
        &mut self,
        focused: usize,
        total_rows: usize,
        visible_rows: usize,
    ) -> JobsViewportWindow {
        if total_rows == 0 || visible_rows == 0 {
            self.offset = 0;
            return JobsViewportWindow::default();
        }
        let focused = focused.min(total_rows - 1);
        let marker_capacity = visible_rows.saturating_sub(1);
        let content_capacity = marker_capacity.max(1);

        if focused < self.offset {
            self.offset = focused;
        } else if focused >= self.offset.saturating_add(content_capacity) {
            self.offset = focused.saturating_add(1).saturating_sub(content_capacity);
        }
        self.offset = self.offset.min(total_rows.saturating_sub(content_capacity));

        let mut start = self.offset;
        let mut end = start.saturating_add(content_capacity).min(total_rows);
        let mut show_above = start > 0;
        let mut show_below = end < total_rows;

        while end.saturating_sub(start) + usize::from(show_above) + usize::from(show_below)
            > visible_rows
        {
            if focused + 1 < end {
                end -= 1;
            } else if start < focused {
                start += 1;
            } else {
                break;
            }
            show_above = start > 0;
            show_below = end < total_rows;
        }
        self.offset = start;
        JobsViewportWindow {
            start,
            end,
            show_above,
            show_below,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{JobsDetail, JobsViewport, display_output_line, grouped_jobs, visible_jobs};
    use crate::shell::jobs::{JobSourceSummary, JobSummary, JobSummaryStatus};

    fn job(id: &str, source_key: &str, status: JobSummaryStatus) -> JobSummary {
        JobSummary {
            id: id.to_string(),
            command: format!("printf {id}"),
            cwd: format!("/{source_key}"),
            started_at: "2026-07-16T10:00:00Z".to_string(),
            status,
            source: JobSourceSummary {
                key: source_key.to_string(),
                tag: source_key.to_string(),
                server_pid: 7,
                parent_executable: Some("codex".to_string()),
                server_cwd: format!("/{source_key}"),
            },
        }
    }

    #[test]
    fn viewport_keeps_the_focused_job_visible_at_both_edges() {
        let mut viewport = JobsViewport::default();
        let top = viewport.window(0, 20, 6);
        assert_eq!((top.start, top.end), (0, 5));
        assert!(!top.show_above);
        assert!(top.show_below);

        let middle = viewport.window(10, 20, 6);
        assert!(middle.start <= 10 && 10 < middle.end);
        assert!(middle.show_above);
        assert!(middle.show_below);

        let bottom = viewport.window(19, 20, 6);
        assert!(bottom.start <= 19 && 19 < bottom.end);
        assert!(bottom.show_above);
        assert!(!bottom.show_below);
    }

    #[test]
    fn output_navigation_is_bounded_and_manual_scrolling_pauses_follow() {
        let mut detail = JobsDetail::default();
        detail.tail.lines = vec!["short".to_string(), "x".repeat(20)];

        detail.move_horizontal(true);
        assert_eq!(detail.horizontal_offset, 8);
        detail.move_horizontal(true);
        detail.move_horizontal(true);
        assert_eq!(detail.horizontal_offset, 19);
        detail.move_horizontal(false);
        assert_eq!(detail.horizontal_offset, 11);

        detail.page_output(false);
        assert!(!detail.follow_tail);
        assert_eq!(detail.lines_below, 8);
        detail.preserve_view_after_append(3);
        assert_eq!(detail.lines_below, 11);
        detail.jump_to_output_edge(true);
        assert!(detail.follow_tail);
        assert_eq!(detail.lines_below, 0);
    }

    #[test]
    fn horizontal_navigation_uses_terminal_columns_for_wide_output() {
        let mut detail = JobsDetail::default();
        detail.tail.lines = vec!["界".repeat(10)];

        detail.move_horizontal(true);
        detail.move_horizontal(true);
        detail.move_horizontal(true);

        assert_eq!(detail.horizontal_offset, 19);
    }

    #[test]
    fn horizontal_navigation_uses_the_visible_width_of_escaped_controls() {
        let mut detail = JobsDetail::default();
        detail.tail.lines = vec!["\t界".to_string()];

        detail.move_horizontal(true);

        assert_eq!(display_output_line("\t界").as_ref(), r"\t界");
        assert_eq!(detail.horizontal_offset, 3);
    }

    #[test]
    fn current_user_snapshot_groups_every_source_and_never_exposes_terminal_records() {
        let jobs = vec![
            job("j-a-run", "source-a", JobSummaryStatus::Running),
            job("j-b-run", "source-b", JobSummaryStatus::Running),
            job("j-a-done", "source-a", JobSummaryStatus::Exited(0)),
            job("j-b-lost", "source-b", JobSummaryStatus::Interrupted),
        ];

        let groups = grouped_jobs(&jobs);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0]
                .jobs
                .iter()
                .map(|job| job.id.as_str())
                .collect::<Vec<_>>(),
            ["j-a-run"]
        );
        assert_eq!(
            groups[1]
                .jobs
                .iter()
                .map(|job| job.id.as_str())
                .collect::<Vec<_>>(),
            ["j-b-run"]
        );
        assert_eq!(
            visible_jobs(&jobs)
                .iter()
                .map(|job| job.id.as_str())
                .collect::<Vec<_>>(),
            ["j-a-run", "j-b-run"]
        );
        assert_eq!(groups[0].total, 1);
        assert_eq!(groups[1].total, 1);
    }
}
