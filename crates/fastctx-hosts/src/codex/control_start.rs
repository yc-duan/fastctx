//! A control's start acknowledgement can precede its turn notification.

use super::CodexHost;
use super::control_error::ControlError;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

impl CodexHost {
    /// Does not release the caller's admission gate until the accepted operation
    /// has actually started, including one whose turn already completed.
    pub(super) async fn start_control(
        self: &Arc<Self>,
        thread: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, ControlError> {
        let server = self.server().await?;
        let before = self.lock().threads.get(thread).map(|entry| entry.start_revision).unwrap_or(0);
        let answer = match server.ask(method, params).await {
            Ok(answer) => answer,
            Err(super::server::Unanswered::Stopped(_))
                if method == "thread/compact/start"
                    && self.lock().threads.get(thread).is_some_and(|entry| entry.start_revision != before) =>
            {
                Value::Null
            }
            Err(error) => return Err(error.into()),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            {
                let mut state = self.lock();
                let entry = state.threads.get(thread).ok_or("the controlled Codex thread disappeared")?;
                if entry.start_revision != before {
                    return Ok(answer);
                }
                if !entry.loaded {
                    return Err(ControlError::uncertain(
                        "Codex unloaded the thread before confirming the control's turn; its outcome is unknown",
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ControlError::uncertain(format!(
                    "Codex accepted {method} but did not report its turn within 120 seconds; its outcome is unknown"
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
