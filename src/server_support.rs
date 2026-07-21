//! Shared MCP server plumbing for bounded blocking work and content conversion.

use crate::budget::{ErrorBudgetAdapter, ErrorClass, error_budget_hint};
use crate::file_executor::GrepGlobExecutor;
use crate::model::{ImageDetail, ToolContent, ToolResponse};
use crate::operation::{OpError, OperationCtx, RequestWorkGuard};
#[cfg(test)]
use crate::operation::{TestStage, TestStageHook};
use rmcp::model::RequestId;
use rmcp::model::{CallToolResult, ContentBlock, ImageContent, Meta};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Runs synchronous tool work behind a shared semaphore and converts its response.
pub(crate) async fn run_blocking(
    permits: Arc<Semaphore>,
    operation: impl FnOnce() -> ToolResponse + Send + 'static,
) -> CallToolResult {
    let permit = match permits.acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return CallToolResult::error(vec![ContentBlock::text(
                "Internal tool failure: the blocking-operation limiter is unavailable.",
            )]);
        }
    };
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    {
        Ok(response) => into_mcp_result(response),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(format!(
            "Internal tool failure: {error}"
        ))]),
    }
}

/// Runs grep/glob work with cancel-aware admission and a drop-cancelled blocking sibling.
pub(crate) async fn run_blocking_cancellable(
    request_id: RequestId,
    request_cancel: CancellationToken,
    permits: Arc<Semaphore>,
    executor: Arc<GrepGlobExecutor>,
    budget_variable: &'static str,
    operation: impl FnOnce(OperationCtx, Arc<GrepGlobExecutor>) -> Result<ToolResponse, OpError>
    + Send
    + 'static,
) -> CallToolResult {
    let (guard, operation_context) = RequestWorkGuard::new(request_id, request_cancel);
    let error_adapter =
        ErrorBudgetAdapter::new(error_budget_hint(budget_variable), budget_variable);
    run_blocking_cancellable_with_context(
        guard,
        operation_context,
        permits,
        executor,
        error_adapter,
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
    operation: impl FnOnce(OperationCtx, Arc<GrepGlobExecutor>) -> Result<ToolResponse, OpError>
    + Send
    + 'static,
) -> CallToolResult {
    let (guard, operation_context) =
        RequestWorkGuard::new_with_hook(request_id, request_cancel, stage_hook);
    let error_adapter =
        ErrorBudgetAdapter::new(error_budget_hint(budget_variable), budget_variable);
    run_blocking_cancellable_with_context(
        guard,
        operation_context,
        permits,
        executor,
        error_adapter,
        operation,
    )
    .await
}

async fn run_blocking_cancellable_with_context(
    mut guard: RequestWorkGuard,
    operation_context: OperationCtx,
    permits: Arc<Semaphore>,
    executor: Arc<GrepGlobExecutor>,
    error_adapter: ErrorBudgetAdapter<'static>,
    operation: impl FnOnce(OperationCtx, Arc<GrepGlobExecutor>) -> Result<ToolResponse, OpError>
    + Send
    + 'static,
) -> CallToolResult {
    #[cfg(test)]
    operation_context.stage(TestStage::BeforeFilePermit);
    let cancellation = operation_context.cancellation_token().clone();
    let permit = tokio::select! {
        _ = cancellation.cancelled() => {
            guard.disarm();
            return into_mcp_result(error_adapter.error(ErrorClass::Cancelled, "Request cancelled."));
        }
        permit = permits.acquire_owned() => match permit {
            Ok(permit) => permit,
            Err(_) => {
                guard.disarm();
                return into_mcp_result(error_adapter.error(
                    ErrorClass::Other,
                    "Internal tool failure: the blocking-operation limiter is unavailable.",
                ));
            }
        }
    };
    #[cfg(test)]
    operation_context.stage(TestStage::AfterFilePermit);
    if let Err(error) = operation_context.check() {
        drop(permit);
        guard.disarm();
        return into_mcp_result(error_adapter.adapt(error.into_response()));
    }

    let completion_context = operation_context.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation_context.check()?;
        let response = operation(operation_context.clone(), executor)?;
        operation_context.check()?;
        Ok::<_, OpError>(response)
    })
    .await;
    let completion_error = completion_context.check().err();
    guard.disarm();
    if let Some(error) = completion_error {
        return into_mcp_result(error_adapter.adapt(error.into_response()));
    }
    match result {
        Ok(Ok(response)) => into_mcp_result(error_adapter.adapt(response)),
        Ok(Err(error)) => into_mcp_result(error_adapter.adapt(error.into_response())),
        Err(error) => into_mcp_result(
            error_adapter.error(ErrorClass::Other, format!("Internal tool failure: {error}")),
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
        into_mcp_result, run_blocking, run_blocking_cancellable, run_blocking_cancellable_with_hook,
    };
    use crate::budget::GREP_TOKEN_BUDGET_ENV;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocking_operations_are_bounded_before_they_reach_tokio() {
        let permits = Arc::new(Semaphore::new(1));
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let first_permits = Arc::clone(&permits);
        let first = tokio::spawn(async move {
            run_blocking(first_permits, move || {
                first_started_tx.send(()).unwrap();
                release_first_rx.recv().unwrap();
                ToolResponse::text("first")
            })
            .await
        });
        first_started_rx.recv().unwrap();

        let (second_waiting_tx, second_waiting_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let second = tokio::spawn(async move {
            second_waiting_tx.send(()).unwrap();
            run_blocking(permits, move || {
                second_started_tx.send(()).unwrap();
                ToolResponse::text("second")
            })
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
            request_id(12),
            parent.clone(),
            Arc::clone(&permits),
            file_executor(),
            GREP_TOKEN_BUDGET_ENV,
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
            request_id(13),
            CancellationToken::new(),
            Arc::clone(&permits),
            file_executor(),
            GREP_TOKEN_BUDGET_ENV,
            move |_, _| -> Result<ToolResponse, OpError> { panic!("injected coordinator panic") },
        )
        .await;
        assert!(error_text(result).starts_with("Internal tool failure: task "));
        assert_eq!(permits.available_permits(), 1);
    }
}
