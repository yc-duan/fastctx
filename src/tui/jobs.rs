//! Aggregated running-job state, output navigation, and bounded viewports.

use super::job_snapshot::JobSnapshot;
use crate::shell::jobs::{JobSummary, JobTail};
use ratatui::text::Line;
use std::borrow::Cow;
use std::sync::Arc;

/// Escapes terminal controls without allocating for ordinary output lines.
pub(crate) fn display_output_line(value: &str) -> Cow<'_, str> {
    if value.chars().all(|character| !character.is_control()) {
        return Cow::Borrowed(value);
    }
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    Cow::Owned(output)
}

/// Complete data model for the current user's cross-session job dashboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum JobsState {
    Loading,
    Ready(Arc<JobSnapshot>),
    Empty,
    PermissionDenied(String),
    Error(String),
}

impl JobsState {
    pub(crate) fn ready(jobs: Vec<JobSummary>) -> Self {
        Self::Ready(Arc::new(JobSnapshot::new(jobs)))
    }

    pub(crate) fn jobs(&self) -> &[JobSummary] {
        match self {
            Self::Ready(snapshot) => &snapshot.jobs,
            Self::Loading | Self::Empty | Self::PermissionDenied(_) | Self::Error(_) => &[],
        }
    }
    pub(crate) fn snapshot(&self) -> Option<&JobSnapshot> {
        match self {
            Self::Ready(snapshot) => Some(snapshot),
            _ => None,
        }
    }

    pub(crate) fn visible_count(&self) -> usize {
        self.snapshot().map_or(0, JobSnapshot::visible_count)
    }
}

/// Read-only output detail and viewport for the focused job.
#[derive(Clone, Debug)]
pub(crate) struct JobsDetail {
    pub(crate) job_id: Option<String>,
    pub(crate) tail: JobTail,
    pub(crate) error: Option<String>,
    pub(crate) loading: bool,
    pub(crate) horizontal_offset: usize,
    pub(crate) lines_below: usize,
    pub(crate) follow_tail: bool,
    max_line_width: Option<usize>,
}

impl Default for JobsDetail {
    fn default() -> Self {
        Self {
            job_id: None,
            tail: JobTail::default(),
            error: None,
            loading: false,
            horizontal_offset: 0,
            lines_below: 0,
            follow_tail: true,
            max_line_width: None,
        }
    }
}

impl JobsDetail {
    pub(crate) fn set_tail(&mut self, tail: JobTail, max_line_width: usize) {
        self.max_line_width = Some(max_line_width);
        self.tail = tail;
    }

    pub(crate) fn max_line_width(&self) -> usize {
        self.max_line_width
            .unwrap_or_else(|| output_width(&self.tail))
    }

    pub(crate) fn move_horizontal(&mut self, forward: bool) {
        const STEP: usize = 8;
        if forward {
            let max_offset = self.max_line_width().saturating_sub(1);
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

pub(super) fn output_width(tail: &JobTail) -> usize {
    tail.lines
        .iter()
        .map(|line| Line::from(display_output_line(line)).width())
        .max()
        .unwrap_or(0)
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
