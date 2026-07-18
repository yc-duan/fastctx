//! Shared MCP server plumbing for bounded blocking work and content conversion.

use crate::model::{ImageDetail, ToolContent, ToolResponse};
use rmcp::model::{CallToolResult, ContentBlock, ImageContent, Meta};
use std::sync::Arc;
use tokio::sync::Semaphore;

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
    use super::{into_mcp_result, run_blocking};
    use crate::{ImageDetail, ToolContent, ToolResponse};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    use tokio::sync::Semaphore;

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
}
