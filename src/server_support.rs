//! Shared MCP server plumbing for bounded blocking work and content conversion.

use crate::background_status::{BackgroundDecorator, BackgroundStatus};
use crate::budget::{
    ErrorBudgetAdapter, ResponseBudgetCeiling, error_budget_hint, estimate_tokens,
};
use crate::context_guard::{BurstClaim, BurstTicket};
use crate::file_executor::GrepGlobExecutor;
use crate::model::{ImageDetail, ToolContent, ToolResponse};
use crate::operation::{OpError, OperationCtx, RequestWorkGuard};
#[cfg(test)]
use crate::operation::{TestStage, TestStageHook};
use crate::session::SessionContext;
use rmcp::model::RequestId;
use rmcp::model::{CallToolResult, ContentBlock, ImageContent, Meta};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

// Admission reservation, not a claim about provider usage accounting. Codex does not expose
// visual-token usage to MCP; 3,000 conservatively covers one high-detail image after host resizing.
const GUARDED_IMAGE_TOKEN_RESERVATION: usize = 3_000;

/// Whether an operation is safe to repeat after status reservation starves its required note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BudgetRetry {
    Never,
    Safe,
}

pub(crate) struct CancellableBlockingRequest {
    session: Arc<SessionContext>,
    request_id: RequestId,
    request_cancel: CancellationToken,
    permits: Arc<Semaphore>,
    executor: Arc<GrepGlobExecutor>,
    budget_variable: &'static str,
}

struct BurstFormatter {
    claim: Option<BurstClaim>,
    ceiling: Option<ResponseBudgetCeiling>,
    budget_variable: &'static str,
}

impl BurstFormatter {
    fn new(ticket: Option<BurstTicket>, budget_variable: &'static str) -> Self {
        let claim = ticket.map(|ticket| ticket.claim(error_budget_hint(budget_variable)));
        let ceiling = claim
            .as_ref()
            .map(|claim| ResponseBudgetCeiling::install(claim.allowance()));
        Self {
            claim,
            ceiling,
            budget_variable,
        }
    }

    fn rendered(mut self, response: ToolResponse) -> PendingBurstResponse {
        self.ceiling.take();
        PendingBurstResponse {
            response,
            claim: self.claim.take(),
            budget_variable: self.budget_variable,
        }
    }
}

struct PendingBurstResponse {
    response: ToolResponse,
    claim: Option<BurstClaim>,
    budget_variable: &'static str,
}

impl PendingBurstResponse {
    fn replace(mut self, response: ToolResponse) -> Self {
        self.response = response;
        self
    }

    fn deliver(mut self) -> ToolResponse {
        let allowance = self.claim.as_ref().map(BurstClaim::allowance);
        if self.claim.as_ref().is_some_and(|claim| claim.exhausted())
            && is_budget_starvation(&self.response)
        {
            self.response = guarded_burst_stub(
                self.budget_variable,
                allowance.unwrap_or_default(),
                &self.response,
            );
        }
        let mut actual_tokens = response_accounted_tokens(&self.response);
        if allowance.is_some_and(|allowance| actual_tokens > allowance) {
            self.response = guarded_burst_stub(
                self.budget_variable,
                allowance.unwrap_or_default(),
                &self.response,
            );
            actual_tokens = response_accounted_tokens(&self.response);
        }
        if let Some(claim) = self.claim.take() {
            claim.complete(actual_tokens);
        }
        self.response
    }
}

fn response_accounted_tokens(response: &ToolResponse) -> usize {
    let text_tokens = response
        .content
        .iter()
        .filter_map(|content| match content {
            ToolContent::Text(text) => Some(estimate_tokens(text)),
            ToolContent::Image { .. } => None,
        })
        .fold(0_usize, usize::saturating_add);
    let image_tokens = response
        .content
        .iter()
        .filter(|content| matches!(content, ToolContent::Image { .. }))
        .count()
        .saturating_mul(GUARDED_IMAGE_TOKEN_RESERVATION);
    text_tokens.saturating_add(image_tokens)
}

fn is_budget_starvation(response: &ToolResponse) -> bool {
    response.is_error
        && response.content.iter().any(|content| {
            matches!(content, ToolContent::Text(text) if {
                let lower = text.to_ascii_lowercase();
                text.contains("TOKEN_BUDGET")
                    || lower.contains("budget too small")
                    || lower.contains("too small to return")
            })
        })
}

fn guarded_burst_stub(
    budget_variable: &'static str,
    allowance: usize,
    response: &ToolResponse,
) -> ToolResponse {
    let retrieval = match budget_variable {
        crate::budget::READ_TOKEN_BUDGET_ENV => {
            "Retry the same inspect_local_file call next turn; continue from any offset in the last Partial response."
        }
        crate::budget::GREP_TOKEN_BUDGET_ENV => {
            "Retry grep next turn with the same request, or narrow path/pattern and continue from any reported offset."
        }
        crate::budget::GLOB_TOKEN_BUDGET_ENV => {
            "Retry glob next turn with the same request and continue from any offset in the last Partial response."
        }
        crate::budget::RUN_TOKEN_BUDGET_ENV => {
            "The command already ran. Do not repeat side effects blindly; run a safe inspection command next turn or redirect a deliberate rerun to a file."
        }
        crate::budget::JOB_OUTPUT_TOKEN_BUDGET_ENV => {
            "Retry job_output next turn with the same job_id and after_seq cursor."
        }
        _ => {
            "Retry the same tool call next turn; its original arguments remain the retrieval path."
        }
    };
    let metadata = guarded_response_metadata(response);
    let full = format!(
        "Guarded burst stub\n- Result: {metadata}\n- State: this call exhausted its share of the FastCtx output pool for the turn; result content was withheld to preserve compaction room.\n- Retrieval: {retrieval}"
    );
    if estimate_tokens(&full) <= allowance {
        return ToolResponse::text(full);
    }
    if let Some(compact) = compact_guarded_stub(&metadata, retrieval, allowance) {
        return ToolResponse::text(compact);
    }

    // A pathological number of calls can consume the absolute 13,599-token ceiling after the
    // normal 9,000-token pool is already empty. At that point no metadata-bearing stub can fit;
    // fail explicitly within the remaining allowance instead of returning a silent empty success.
    let emergency = [
        "Guarded burst limit reached; retry next turn.",
        "Retry next turn.",
        "Limit.",
        "",
    ]
    .into_iter()
    .find(|candidate| estimate_tokens(candidate) <= allowance)
    .unwrap_or_default();
    ToolResponse::error(emergency)
}

fn compact_guarded_stub(metadata: &str, retrieval: &str, allowance: usize) -> Option<String> {
    let mut prefix = metadata.chars().take(96).collect::<String>();
    loop {
        let ellipsis = if prefix.len() < metadata.len() {
            "…"
        } else {
            ""
        };
        let candidate =
            format!("Guarded burst withheld this result ({prefix}{ellipsis}). {retrieval}");
        if !prefix.is_empty() && estimate_tokens(&candidate) <= allowance {
            return Some(candidate);
        }
        prefix.pop()?;
    }
}

fn guarded_response_metadata(response: &ToolResponse) -> String {
    let image_count = response
        .content
        .iter()
        .filter(|content| matches!(content, ToolContent::Image { .. }))
        .count();
    let terminal = response.content.iter().rev().find_map(|content| {
        let ToolContent::Text(text) = content else {
            return None;
        };
        text.lines()
            .rev()
            .find(|line| {
                let line = line.trim();
                line.starts_with("(Complete:")
                    || line.starts_with("(Partial:")
                    || line.starts_with("(Killed:")
                    || line.starts_with("Script running with cell ID")
            })
            .map(str::trim)
    });
    match (terminal, image_count) {
        (Some(terminal), 0) => terminal.to_string(),
        (Some(terminal), count) => format!("{terminal}; {count} image block(s)"),
        (None, 0) if response.is_error => "the tool completed with an error".to_string(),
        (None, 0) => "the tool completed successfully".to_string(),
        (None, count) => format!("the tool completed with {count} image block(s)"),
    }
}

fn finish_early_response(
    session: &Arc<SessionContext>,
    ticket: Option<BurstTicket>,
    budget_variable: &'static str,
    response: ToolResponse,
) -> CallToolResult {
    let response = session.activate(|| {
        let formatter = BurstFormatter::new(ticket, budget_variable);
        let adapter = ErrorBudgetAdapter::new(error_budget_hint(budget_variable), budget_variable);
        formatter.rendered(adapter.adapt(response)).deliver()
    });
    into_mcp_result(response)
}

fn await_test_tool_barrier() {
    #[cfg(debug_assertions)]
    if let Ok(directory) = crate::session::var("FASTCTX_TEST_TOOL_BARRIER_DIR") {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static NEXT_PARTICIPANT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::path::PathBuf::from(directory);
        let participant = NEXT_PARTICIPANT.fetch_add(1, Ordering::Relaxed);
        std::fs::write(directory.join(format!("participant-{participant}")), [])
            .expect("the guarded-burst test barrier must accept participant markers");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let participants = std::fs::read_dir(&directory)
                .expect("the guarded-burst test barrier must remain readable")
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("participant-")
                })
                .count();
            if participants >= 2 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "two guarded tool calls did not reach blocking work concurrently"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl CancellableBlockingRequest {
    pub(crate) fn new(
        session: Arc<SessionContext>,
        request_id: RequestId,
        request_cancel: CancellationToken,
        permits: Arc<Semaphore>,
        executor: Arc<GrepGlobExecutor>,
        budget_variable: &'static str,
    ) -> Self {
        Self {
            session,
            request_id,
            request_cancel,
            permits,
            executor,
            budget_variable,
        }
    }
}

/// Runs synchronous tool work behind a shared semaphore and converts its response.
pub(crate) async fn run_blocking(
    session: Arc<SessionContext>,
    permits: Arc<Semaphore>,
    budget_variable: &'static str,
    status: impl FnOnce() -> Option<BackgroundStatus> + Send + 'static,
    retry: BudgetRetry,
    mut operation: impl FnMut() -> ToolResponse + Send + 'static,
) -> CallToolResult {
    let burst_ticket = session.begin_guarded_response();
    let permit = match permits.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return finish_early_response(
                &session,
                burst_ticket,
                budget_variable,
                ToolResponse::error(
                    "Internal tool failure: the blocking-operation limiter is unavailable.",
                ),
            );
        }
    };
    let failure_session = Arc::clone(&session);
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        session.activate(|| {
            await_test_tool_barrier();
            let burst = BurstFormatter::new(burst_ticket, budget_variable);
            let decorator = BackgroundDecorator::new(status(), budget_variable);
            let response = loop {
                let response = operation();
                if retry == BudgetRetry::Safe && decorator.retry_after_budget_starvation(&response)
                {
                    continue;
                }
                break decorator.finish(response);
            };
            burst.rendered(response).deliver()
        })
    })
    .await
    {
        Ok(response) => into_mcp_result(response),
        Err(error) => {
            let ticket = failure_session.begin_guarded_response();
            finish_early_response(
                &failure_session,
                ticket,
                budget_variable,
                ToolResponse::error(format!("Internal tool failure: {error}")),
            )
        }
    }
}

/// Runs grep/glob work with cancel-aware admission and a drop-cancelled blocking sibling.
pub(crate) async fn run_blocking_cancellable(
    request: CancellableBlockingRequest,
    status: impl FnOnce() -> Option<BackgroundStatus> + Send + 'static,
    operation: impl FnMut(OperationCtx, Arc<GrepGlobExecutor>) -> Result<ToolResponse, OpError>
    + Send
    + 'static,
) -> CallToolResult {
    let CancellableBlockingRequest {
        session,
        request_id,
        request_cancel,
        permits,
        executor,
        budget_variable,
    } = request;
    let (guard, operation_context) = RequestWorkGuard::new(request_id, request_cancel);
    let burst_ticket = session.begin_guarded_response();
    run_blocking_cancellable_with_context(
        guard,
        operation_context,
        CancellableBlockingResources {
            session,
            permits,
            executor,
            burst_ticket,
            budget_variable,
        },
        status,
        operation,
    )
    .await
}

#[cfg(test)]
async fn run_blocking_cancellable_with_hook(
    request_id: RequestId,
    request_cancel: CancellationToken,
    permits: Arc<Semaphore>,
    executor: Arc<GrepGlobExecutor>,
    budget_variable: &'static str,
    stage_hook: TestStageHook,
    operation: impl FnMut(OperationCtx, Arc<GrepGlobExecutor>) -> Result<ToolResponse, OpError>
    + Send
    + 'static,
) -> CallToolResult {
    let session = SessionContext::library_default();
    let (guard, operation_context) =
        RequestWorkGuard::new_with_hook(request_id, request_cancel, stage_hook);
    let burst_ticket = session.begin_guarded_response();
    run_blocking_cancellable_with_context(
        guard,
        operation_context,
        CancellableBlockingResources {
            session,
            permits,
            executor,
            burst_ticket,
            budget_variable,
        },
        || None,
        operation,
    )
    .await
}

struct CancellableBlockingResources {
    session: Arc<SessionContext>,
    permits: Arc<Semaphore>,
    executor: Arc<GrepGlobExecutor>,
    burst_ticket: Option<BurstTicket>,
    budget_variable: &'static str,
}

async fn run_blocking_cancellable_with_context(
    mut guard: RequestWorkGuard,
    operation_context: OperationCtx,
    resources: CancellableBlockingResources,
    status: impl FnOnce() -> Option<BackgroundStatus> + Send + 'static,
    mut operation: impl FnMut(OperationCtx, Arc<GrepGlobExecutor>) -> Result<ToolResponse, OpError>
    + Send
    + 'static,
) -> CallToolResult {
    let CancellableBlockingResources {
        session,
        permits,
        executor,
        mut burst_ticket,
        budget_variable,
    } = resources;
    #[cfg(test)]
    operation_context.stage(TestStage::BeforeFilePermit);
    let cancellation = operation_context.cancellation_token().clone();
    let permit = tokio::select! {
        _ = cancellation.cancelled() => {
            guard.disarm();
            return finish_early_response(
                &session,
                burst_ticket.take(),
                budget_variable,
                ToolResponse::error("Request cancelled."),
            );
        }
        permit = permits.acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => {
                guard.disarm();
                return finish_early_response(
                    &session,
                    burst_ticket.take(),
                    budget_variable,
                    ToolResponse::error(
                        "Internal tool failure: the blocking-operation limiter is unavailable.",
                    ),
                );
            }
        }
    };
    #[cfg(test)]
    operation_context.stage(TestStage::AfterFilePermit);
    if let Err(error) = operation_context.check() {
        drop(permit);
        guard.disarm();
        return finish_early_response(
            &session,
            burst_ticket.take(),
            budget_variable,
            error.into_response(),
        );
    }

    let completion_context = operation_context.clone();
    let failure_session = Arc::clone(&session);
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        session.activate(|| {
            await_test_tool_barrier();
            let burst = BurstFormatter::new(burst_ticket, budget_variable);
            let error_adapter =
                ErrorBudgetAdapter::new(error_budget_hint(budget_variable), budget_variable);
            let decorator = BackgroundDecorator::new(status(), budget_variable);
            let response = loop {
                if let Err(error) = operation_context.check() {
                    break error_adapter.adapt(error.into_response());
                }
                let response = match operation(operation_context.clone(), Arc::clone(&executor)) {
                    Ok(response) => response,
                    Err(error) => break error_adapter.adapt(error.into_response()),
                };
                if let Err(error) = operation_context.check() {
                    break error_adapter.adapt(error.into_response());
                }
                if decorator.retry_after_budget_starvation(&response) {
                    continue;
                }
                break decorator.finish(response);
            };
            burst.rendered(response)
        })
    })
    .await;
    let completion_error = completion_context.check().err();
    guard.disarm();
    if let Some(error) = completion_error {
        return match result {
            Ok(pending) => into_mcp_result(pending.replace(error.into_response()).deliver()),
            Err(_) => finish_early_response(
                &failure_session,
                failure_session.begin_guarded_response(),
                budget_variable,
                error.into_response(),
            ),
        };
    }
    match result {
        Ok(pending) => into_mcp_result(pending.deliver()),
        Err(error) => finish_early_response(
            &failure_session,
            failure_session.begin_guarded_response(),
            budget_variable,
            ToolResponse::error(format!("Internal tool failure: {error}")),
        ),
    }
}

/// Converts the protocol-independent response without ever adding structured content.
pub(crate) fn into_mcp_result(response: ToolResponse) -> CallToolResult {
    let content = response
        .content
        .into_iter()
        .map(|block| match block {
            ToolContent::Text(text) => ContentBlock::text(text),
            ToolContent::Image {
                data,
                mime_type,
                detail,
            } => {
                let image = ImageContent::new(data, mime_type);
                if detail == Some(ImageDetail::High) {
                    let mut meta = Meta::new();
                    meta.0.insert(
                        "codex/imageDetail".to_string(),
                        serde_json::Value::String("high".to_string()),
                    );
                    ContentBlock::Image(image.with_meta(meta))
                } else {
                    ContentBlock::Image(image)
                }
            }
        })
        .collect::<Vec<_>>();
    if response.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BudgetRetry, CancellableBlockingRequest, GUARDED_IMAGE_TOKEN_RESERVATION, into_mcp_result,
        response_accounted_tokens, run_blocking, run_blocking_cancellable,
        run_blocking_cancellable_with_hook,
    };
    use crate::budget::{GLOBAL_TOKEN_BUDGET_ENV, GREP_TOKEN_BUDGET_ENV};
    use crate::file_executor::GrepGlobExecutor;
    use crate::operation::{OpError, TestStage};
    use crate::{ImageDetail, ToolContent, ToolResponse};
    use rmcp::model::RequestId;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tokio_util::sync::CancellationToken;

    fn request_id(value: i64) -> RequestId {
        RequestId::Number(value)
    }

    fn file_executor() -> Arc<GrepGlobExecutor> {
        Arc::new(GrepGlobExecutor::new())
    }

    fn error_text(result: rmcp::model::CallToolResult) -> String {
        assert_eq!(result.is_error, Some(true));
        let value = serde_json::to_value(result).unwrap();
        value["content"][0]["text"].as_str().unwrap().to_string()
    }

    #[test]
    fn pdf_image_detail_is_preserved_in_mcp_meta_without_structured_content() {
        let result = into_mcp_result(ToolResponse {
            content: vec![ToolContent::Image {
                data: "AA==".to_string(),
                mime_type: "image/png".to_string(),
                detail: Some(ImageDetail::High),
            }],
            is_error: false,
        });
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["content"][0]["_meta"]["codex/imageDetail"], "high");
        assert!(value.get("structuredContent").is_none());
    }

    #[test]
    fn guarded_accounting_combines_text_with_every_image_reservation() {
        let response = ToolResponse {
            content: std::iter::once(ToolContent::Text("image metadata".to_string()))
                .chain((0..4).map(|_| ToolContent::Image {
                    data: "AA==".to_string(),
                    mime_type: "image/png".to_string(),
                    detail: Some(ImageDetail::High),
                }))
                .collect(),
            is_error: false,
        };
        let accounted = response_accounted_tokens(&response);
        assert!(accounted > 4 * GUARDED_IMAGE_TOKEN_RESERVATION);
        assert!(accounted > crate::control::provider::GUARDED_FASTCTX_BUDGET);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_operations_are_bounded_before_they_reach_tokio() {
        let permits = Arc::new(Semaphore::new(1));
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let first_permits = Arc::clone(&permits);
        let first = tokio::spawn(async move {
            run_blocking(
                crate::session::SessionContext::library_default(),
                first_permits,
                GLOBAL_TOKEN_BUDGET_ENV,
                || None,
                BudgetRetry::Never,
                move || {
                    first_started_tx.send(()).unwrap();
                    release_first_rx.recv().unwrap();
                    ToolResponse::text("first")
                },
            )
            .await
        });
        first_started_rx.recv().unwrap();

        let (second_waiting_tx, second_waiting_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let second = tokio::spawn(async move {
            second_waiting_tx.send(()).unwrap();
            run_blocking(
                crate::session::SessionContext::library_default(),
                permits,
                GLOBAL_TOKEN_BUDGET_ENV,
                || None,
                BudgetRetry::Never,
                move || {
                    second_started_tx.send(()).unwrap();
                    ToolResponse::text("second")
                },
            )
            .await
        });
        second_waiting_rx.recv().unwrap();
        assert!(matches!(
            second_started_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_first_tx.send(()).unwrap();
        first.await.unwrap();
        second.await.unwrap();
        second_started_rx.recv().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_cancellation_never_starts_blocking_work_or_leaks_a_permit() {
        let permits = Arc::new(Semaphore::new(1));
        let held = Arc::clone(&permits).acquire_owned().await.unwrap();
        let request_cancel = CancellationToken::new();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let hook = Arc::new(move |stage| {
            if stage == TestStage::BeforeFilePermit {
                waiting_tx.send(()).unwrap();
            }
        });
        let task = tokio::spawn(run_blocking_cancellable_with_hook(
            request_id(10),
            request_cancel.clone(),
            Arc::clone(&permits),
            file_executor(),
            GREP_TOKEN_BUDGET_ENV,
            hook,
            move |_, _| {
                started_tx.send(()).unwrap();
                Ok(ToolResponse::text("unexpected"))
            },
        ));
        waiting_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        request_cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(error_text(result), "Request cancelled.");
        assert!(matches!(
            started_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(permits.available_permits(), 0);
        drop(held);
        assert_eq!(permits.available_permits(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_after_admission_cannot_cross_into_the_coordinator() {
        let permits = Arc::new(Semaphore::new(1));
        let request_cancel = CancellationToken::new();
        let hook_cancel = request_cancel.clone();
        let hook = Arc::new(move |stage| {
            if stage == TestStage::AfterFilePermit {
                hook_cancel.cancel();
            }
        });
        let (started_tx, started_rx) = mpsc::channel();
        let result = run_blocking_cancellable_with_hook(
            request_id(11),
            request_cancel,
            Arc::clone(&permits),
            file_executor(),
            GREP_TOKEN_BUDGET_ENV,
            hook,
            move |_, _| {
                started_tx.send(()).unwrap();
                Ok(ToolResponse::text("unexpected"))
            },
        )
        .await;
        assert_eq!(error_text(result), "Request cancelled.");
        assert!(matches!(
            started_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(permits.available_permits(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_async_future_cancels_started_blocking_work() {
        let permits = Arc::new(Semaphore::new(1));
        let parent = CancellationToken::new();
        let (started_tx, started_rx) = mpsc::channel();
        let (cancelled_tx, cancelled_rx) = mpsc::channel();
        let task = tokio::spawn(run_blocking_cancellable(
            CancellableBlockingRequest::new(
                crate::session::SessionContext::library_default(),
                request_id(12),
                parent.clone(),
                Arc::clone(&permits),
                file_executor(),
                GREP_TOKEN_BUDGET_ENV,
            ),
            || None,
            move |operation, _| {
                started_tx.send(()).unwrap();
                loop {
                    if operation.check() == Err(OpError::Cancelled) {
                        cancelled_tx.send(()).unwrap();
                        return Err(OpError::Cancelled);
                    }
                    std::thread::yield_now();
                }
            },
        ));
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        cancelled_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while permits.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!parent.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_coordinator_returns_the_file_permit() {
        let permits = Arc::new(Semaphore::new(1));
        let result = run_blocking_cancellable(
            CancellableBlockingRequest::new(
                crate::session::SessionContext::library_default(),
                request_id(13),
                CancellationToken::new(),
                Arc::clone(&permits),
                file_executor(),
                GREP_TOKEN_BUDGET_ENV,
            ),
            || None,
            move |_, _| -> Result<ToolResponse, OpError> { panic!("injected coordinator panic") },
        )
        .await;
        assert!(error_text(result).starts_with("Internal tool failure: task "));
        assert_eq!(permits.available_permits(), 1);
    }
}
