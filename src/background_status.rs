//! Per-response background-job status rendering and budget-safe insertion.

use crate::budget::{ResponseReservation, ResponseReservationOutcome, estimate_tokens};
use crate::{ToolContent, ToolResponse};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

type TerminalAcknowledger = Arc<dyn Fn(&[String]) + Send + Sync>;

/// Lifecycle displayed for one job known to the current MCP session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BackgroundState {
    Running,
    Exited(i32),
    Killed,
    Interrupted,
}

/// One status entry with the persisted start time used for sorting and elapsed rendering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BackgroundEntry {
    pub(crate) job_id: String,
    pub(crate) started_at: SystemTime,
    pub(crate) started_sort_key: u64,
    pub(crate) state: BackgroundState,
}

/// Fully rendered full and summary forms for one response instant.
#[derive(Clone)]
pub(crate) struct BackgroundStatus {
    full_line: String,
    summary_line: String,
    terminal_ids: Vec<String>,
    acknowledge: Option<TerminalAcknowledger>,
}

impl BackgroundStatus {
    pub(crate) fn render(mut entries: Vec<BackgroundEntry>, now: SystemTime) -> Option<Self> {
        if entries.is_empty() {
            return None;
        }
        entries.sort_by(|left, right| {
            left.started_sort_key
                .cmp(&right.started_sort_key)
                .then_with(|| left.job_id.as_bytes().cmp(right.job_id.as_bytes()))
        });
        let running_count = entries
            .iter()
            .filter(|entry| entry.state == BackgroundState::Running)
            .count();
        let terminal_ids = entries
            .iter()
            .filter(|entry| entry.state != BackgroundState::Running)
            .map(|entry| entry.job_id.clone())
            .collect();
        let rendered = entries
            .into_iter()
            .map(|entry| match entry.state {
                BackgroundState::Running => format!(
                    "{} running {}",
                    entry.job_id,
                    format_elapsed(now.duration_since(entry.started_at).unwrap_or_default())
                ),
                BackgroundState::Exited(code) => format!("{} exited {code}", entry.job_id),
                BackgroundState::Killed => format!("{} killed", entry.job_id),
                BackgroundState::Interrupted => format!("{} interrupted", entry.job_id),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let running_noun = if running_count == 1 { "job" } else { "jobs" };
        Some(Self {
            full_line: format!("=== jobs: {rendered} ==="),
            summary_line: format!("=== jobs: {running_count} {running_noun} running ==="),
            terminal_ids,
            acknowledge: None,
        })
    }

    pub(crate) fn with_acknowledger(
        mut self,
        acknowledge: impl Fn(&[String]) + Send + Sync + 'static,
    ) -> Self {
        self.acknowledge = Some(Arc::new(acknowledge));
        self
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{}m", seconds / 3_600, (seconds % 3_600) / 60)
    }
}

/// Reservation plus final response insertion for one tool call.
pub(crate) struct BackgroundDecorator {
    reservation: Option<ResponseReservation>,
    terminal_ids: Vec<String>,
    acknowledge: Option<TerminalAcknowledger>,
}

impl BackgroundDecorator {
    pub(crate) fn new(status: Option<BackgroundStatus>, budget_variable: &'static str) -> Self {
        let Some(status) = status else {
            return Self {
                reservation: None,
                terminal_ids: Vec::new(),
                acknowledge: None,
            };
        };
        let reservation =
            ResponseReservation::install(budget_variable, status.full_line, status.summary_line);
        Self {
            reservation,
            terminal_ids: status.terminal_ids,
            acknowledge: status.acknowledge,
        }
    }

    #[cfg(test)]
    fn with_budget(status: BackgroundStatus, budget_variable: &'static str, budget: usize) -> Self {
        Self {
            reservation: Some(ResponseReservation::install_for_test(
                budget_variable,
                budget,
                status.full_line,
                status.summary_line,
            )),
            terminal_ids: status.terminal_ids,
            acknowledge: status.acknowledge,
        }
    }

    /// Returns true when a retry-safe formatter should run again with the next status tier.
    pub(crate) fn retry_after_budget_starvation(&self, response: &ToolResponse) -> bool {
        self.reservation
            .as_ref()
            .is_some_and(|reservation| is_budget_starvation(response) && reservation.downgrade())
    }

    pub(crate) fn finish(self, response: ToolResponse) -> ToolResponse {
        let Some(reservation) = self.reservation else {
            return response;
        };
        let outcome = reservation.finish();
        let (response, delivered) = decorate_response(response, outcome);
        if delivered
            && !self.terminal_ids.is_empty()
            && let Some(acknowledge) = self.acknowledge
        {
            acknowledge(&self.terminal_ids);
        }
        response
    }
}

fn is_budget_starvation(response: &ToolResponse) -> bool {
    response.is_error
        && response.content.iter().any(|content| {
            matches!(content, ToolContent::Text(text) if text.to_ascii_lowercase().contains("too small to return"))
        })
}

fn decorate_response(
    response: ToolResponse,
    outcome: ResponseReservationOutcome,
) -> (ToolResponse, bool) {
    if response.is_error {
        return (response, false);
    }
    let Some(mut line) = outcome.line else {
        return (response, false);
    };
    let Some(index) = response
        .content
        .iter()
        .position(|content| matches!(content, ToolContent::Text(_)))
    else {
        return (response, false);
    };
    loop {
        let mut candidate = response.clone();
        let ToolContent::Text(text) = &mut candidate.content[index] else {
            unreachable!("the selected content block is text")
        };
        insert_status_line(text, &line);
        let tokens = candidate
            .content
            .iter()
            .filter_map(|content| match content {
                ToolContent::Text(text) => Some(estimate_tokens(text)),
                ToolContent::Image { .. } => None,
            })
            .fold(0_usize, usize::saturating_add);
        if tokens <= outcome.configured_budget {
            return (candidate, true);
        }
        // The reservation counts separators conservatively, so this is only a final guard
        // against multi-block totals or tokenizer boundary effects. User content wins.
        if line != outcome.summary_line {
            line.clone_from(&outcome.summary_line);
        } else {
            return (response, false);
        }
    }
}

fn insert_status_line(text: &mut String, status: &str) {
    if text.is_empty() {
        text.push_str(status);
    } else if let Some(first_break) = text.find('\n') {
        text.insert_str(first_break.saturating_add(1), &format!("{status}\n"));
    } else {
        text.push('\n');
        text.push_str(status);
    }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundEntry, BackgroundState, BackgroundStatus, insert_status_line};
    use crate::background_status::BackgroundDecorator;
    use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, estimate_tokens, token_budget};
    use crate::{ImageDetail, ToolContent, ToolResponse};
    use std::time::{Duration, SystemTime};

    fn entry(id: &str, age: u64, state: BackgroundState, now: SystemTime) -> BackgroundEntry {
        BackgroundEntry {
            job_id: id.to_string(),
            started_at: now - Duration::from_secs(age),
            started_sort_key: u64::MAX - age,
            state,
        }
    }

    #[test]
    fn elapsed_boundaries_and_lifecycle_forms_are_byte_exact() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let cases = [
            (59, "j running 59s"),
            (60, "j running 1m0s"),
            (3_599, "j running 59m59s"),
            (3_600, "j running 1h0m"),
        ];
        for (age, expected) in cases {
            let status =
                BackgroundStatus::render(vec![entry("j", age, BackgroundState::Running, now)], now)
                    .unwrap();
            assert_eq!(status.full_line, format!("=== jobs: {expected} ==="));
            assert_eq!(status.summary_line, "=== jobs: 1 job running ===");
        }
        let status = BackgroundStatus::render(
            vec![
                entry("exit", 5, BackgroundState::Exited(42), now),
                entry("lost", 4, BackgroundState::Interrupted, now),
            ],
            now,
        )
        .unwrap();
        assert_eq!(
            status.full_line,
            "=== jobs: exit exited 42, lost interrupted ==="
        );
        assert_eq!(status.summary_line, "=== jobs: 0 jobs running ===");
        assert!(!status.full_line.contains("line"));
        assert!(!status.full_line.contains("byte"));
    }

    #[test]
    fn status_sits_immediately_after_the_response_head() {
        let mut text = "=== fixture (2 lines) ===\nfirst\nsecond".to_string();
        insert_status_line(&mut text, "=== jobs: j running 1s ===");
        assert_eq!(
            text,
            "=== fixture (2 lines) ===\n=== jobs: j running 1s ===\nfirst\nsecond"
        );
        assert_eq!(text.lines().last(), Some("second"));
    }

    fn synthetic_status() -> BackgroundStatus {
        BackgroundStatus {
            full_line: "=== jobs: j-alpha running 12s, j-beta exited 7 ===".to_string(),
            summary_line: "=== jobs: 1 job running ===".to_string(),
            terminal_ids: vec!["j-beta".to_string()],
            acknowledge: None,
        }
    }

    fn budget_error(value: usize) -> ToolResponse {
        ToolResponse::error(format!(
            "FASTCTX_TOKEN_BUDGET={value} is too small to return the required status note. Increase it and retry."
        ))
    }

    fn render_user_lines(budget: usize) -> ToolResponse {
        let head = "=== fixture (output) ===";
        let mut lines = Vec::new();
        for index in 0..1_000 {
            let mut candidate = lines.clone();
            candidate.push(format!("user-{index:04}-payload"));
            let text = format!("{head}\n{}", candidate.join("\n"));
            if estimate_tokens(&text) > budget {
                break;
            }
            lines = candidate;
        }
        ToolResponse::text(format!("{head}\n{}", lines.join("\n")))
    }

    #[test]
    fn full_status_is_prepaid_and_reduces_the_body_before_formatting() {
        let configured = 96;
        let without_status = render_user_lines(configured);
        let decorator = BackgroundDecorator::with_budget(
            synthetic_status(),
            GLOBAL_TOKEN_BUDGET_ENV,
            configured,
        );
        let effective = token_budget().unwrap();
        assert!(effective < configured);
        let with_status = decorator.finish(render_user_lines(effective));
        let ToolContent::Text(without_text) = &without_status.content[0] else {
            panic!("expected text")
        };
        let ToolContent::Text(with_text) = &with_status.content[0] else {
            panic!("expected text")
        };
        assert!(with_text.contains("j-alpha running 12s"));
        assert!(with_text.matches("user-").count() < without_text.matches("user-").count());
        assert!(estimate_tokens(with_text) <= configured);
    }

    #[test]
    fn reservation_degrades_full_to_summary_then_omits_before_reformatting() {
        let status = synthetic_status();
        let full_cost = estimate_tokens(&status.full_line) + estimate_tokens("\n\n");
        let summary_cost = estimate_tokens(&status.summary_line) + estimate_tokens("\n\n");
        assert!(full_cost > summary_cost);

        let required = 8;
        let summary_budget = full_cost + required - 1;
        assert!(summary_budget >= summary_cost + required);
        let decorator = BackgroundDecorator::with_budget(
            status.clone(),
            GLOBAL_TOKEN_BUDGET_ENV,
            summary_budget,
        );
        let mut observed = Vec::new();
        let response = loop {
            let available = token_budget().unwrap();
            observed.push(available);
            let response = if available < required {
                budget_error(available)
            } else {
                ToolResponse::text("=== fixture (output) ===\nuser result")
            };
            if decorator.retry_after_budget_starvation(&response) {
                continue;
            }
            break decorator.finish(response);
        };
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text")
        };
        assert_eq!(observed.len(), 2);
        assert!(observed[0] < observed[1]);
        assert!(text.contains("=== jobs: 1 job running ==="));
        assert!(!text.contains("j-alpha"));
        assert!(estimate_tokens(text) <= summary_budget);

        let omit_required = summary_budget - summary_cost + 1;
        let decorator =
            BackgroundDecorator::with_budget(status, GLOBAL_TOKEN_BUDGET_ENV, summary_budget);
        let response = loop {
            let available = token_budget().unwrap();
            let response = if available < omit_required {
                budget_error(available)
            } else {
                ToolResponse::text("=== fixture (output) ===\nuser result")
            };
            if decorator.retry_after_budget_starvation(&response) {
                continue;
            }
            break decorator.finish(response);
        };
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text")
        };
        assert_eq!(text, "=== fixture (output) ===\nuser result");
    }

    #[test]
    fn delivered_summary_acknowledges_terminal_jobs_but_an_omitted_line_does_not() {
        use std::sync::{Arc, Mutex};

        let acknowledged = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = Arc::clone(&acknowledged);
        let status = synthetic_status().with_acknowledger(move |ids| {
            observed.lock().unwrap().extend_from_slice(ids);
        });
        let decorator = BackgroundDecorator::with_budget(status, GLOBAL_TOKEN_BUDGET_ENV, 256);
        assert!(decorator.retry_after_budget_starvation(&budget_error(1)));
        let response = decorator.finish(ToolResponse::text("=== fixture (output) ===\nbody"));
        let ToolContent::Text(text) = &response.content[0] else {
            panic!("expected text")
        };
        assert!(text.contains("=== jobs: 1 job running ==="));
        assert_eq!(&*acknowledged.lock().unwrap(), &["j-beta".to_string()]);

        let acknowledged = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed = Arc::clone(&acknowledged);
        let status = synthetic_status().with_acknowledger(move |ids| {
            observed.lock().unwrap().extend_from_slice(ids);
        });
        let decorator = BackgroundDecorator::with_budget(status, GLOBAL_TOKEN_BUDGET_ENV, 1);
        let response = decorator.finish(ToolResponse::text("x"));
        assert_eq!(response, ToolResponse::text("x"));
        assert!(acknowledged.lock().unwrap().is_empty());
    }

    #[test]
    fn an_exact_fit_status_cost_yields_to_user_content() {
        let status = synthetic_status();
        let full_cost = estimate_tokens(&status.full_line) + estimate_tokens("\n\n");
        let summary_cost = estimate_tokens(&status.summary_line) + estimate_tokens("\n\n");

        let decorator =
            BackgroundDecorator::with_budget(status.clone(), GLOBAL_TOKEN_BUDGET_ENV, full_cost);
        let summary_response = decorator.finish(ToolResponse::text("user result"));
        let ToolContent::Text(summary_text) = &summary_response.content[0] else {
            panic!("expected text")
        };
        assert!(summary_text.starts_with("user result"));
        assert!(summary_text.contains("=== jobs: 1 job running ==="));
        assert!(!summary_text.contains("j-alpha"));
        assert!(estimate_tokens(summary_text) <= full_cost);

        let decorator =
            BackgroundDecorator::with_budget(status, GLOBAL_TOKEN_BUDGET_ENV, summary_cost);
        let omitted_response = decorator.finish(ToolResponse::text("user result"));
        assert_eq!(omitted_response, ToolResponse::text("user result"));
    }

    #[test]
    fn error_and_image_only_responses_never_receive_background_text() {
        let status = synthetic_status();
        let error =
            BackgroundDecorator::with_budget(status.clone(), GLOBAL_TOKEN_BUDGET_ENV, 8_500)
                .finish(ToolResponse::error("failure"));
        assert_eq!(error, ToolResponse::error("failure"));

        let image = ToolResponse {
            content: vec![ToolContent::Image {
                data: "AA==".to_string(),
                mime_type: "image/png".to_string(),
                detail: Some(ImageDetail::High),
            }],
            is_error: false,
        };
        let decorated = BackgroundDecorator::with_budget(status, GLOBAL_TOKEN_BUDGET_ENV, 8_500)
            .finish(image.clone());
        assert_eq!(decorated, image);
    }
}
