//! Stable lifecycle RPCs. Retiring a subscription and unloading a thread
//! are different facts: other clients and Codex's idle delay still apply.

use super::CodexHost;
use serde_json::json;
use std::sync::Arc;

impl CodexHost {
    pub fn loaded(&self, thread: &str) -> bool {
        self.lock().threads.get(thread).is_some_and(|entry| entry.loaded)
    }

    /// Returns true only once unloading is observed. False means this
    /// connection unsubscribed but Codex still holds the runtime.
    pub async fn sleep(self: &Arc<Self>, thread: &str) -> Result<bool, String> {
        let server = self.server().await?;
        let _loading = self.loading.lock().await;
        if self.lock().threads.get(thread).is_some_and(|entry| entry.turn.is_some() || entry.submitted) {
            return Err("this Codex thread is running a turn and cannot sleep".into());
        }
        let answer = server.request("thread/unsubscribe", json!({"threadId":thread})).await?;
        let status = answer.get("status").and_then(serde_json::Value::as_str);
        if !matches!(status, Some("unsubscribed" | "notSubscribed" | "notLoaded")) {
            return Err(format!("thread/unsubscribe returned an unknown result: {answer}"));
        }
        let mut state = self.lock();
        if let Some(entry) = state.threads.get_mut(thread) {
            entry.subscribed = false;
            if status == Some("notLoaded") {
                entry.loaded = false;
            }
            return Ok(!entry.loaded);
        }
        Ok(status == Some("notLoaded"))
    }

    /// Deletes a thread with everything Codex keeps of it: its records, its
    /// history and the threads of the subagents it spawned
    /// (`thread/delete`). A thread running a turn is not deleted.
    pub async fn delete(self: &Arc<Self>, thread: &str) -> Result<bool, String> {
        let server = self.server().await?;
        let _loading = self.loading.lock().await;
        if self.lock().threads.get(thread).is_some_and(|entry| entry.turn.is_some() || entry.submitted) {
            return Err("this Codex thread is running a turn".into());
        }
        let deleted = match server.ask("thread/delete", json!({"threadId":thread})).await {
            Ok(_) => true,
            Err(super::server::Unanswered::Rpc { code: -32600, message, .. })
                if message == format!("thread not found: {thread}") =>
            {
                false
            }
            Err(error) => return Err(error.to_string()),
        };
        self.lock().threads.remove(thread);
        Ok(deleted)
    }

    /// Archiving also closes a loaded runtime. Failure may follow closure;
    /// keep the official error and the host's reported events intact.
    pub async fn archive(self: &Arc<Self>, thread: &str) -> Result<(), String> {
        let server = self.server().await?;
        let _loading = self.loading.lock().await;
        server.request("thread/archive", json!({"threadId":thread})).await?;
        if let Some(entry) = self.lock().threads.get_mut(thread) {
            entry.loaded = false;
            entry.subscribed = false;
        }
        self.flush_events(thread).await
    }

    /// Takes an archived thread out of the archive (`thread/unarchive`);
    /// it is loaded again when it is next spoken to.
    pub async fn unarchive(self: &Arc<Self>, thread: &str) -> Result<(), String> {
        let server = self.server().await?;
        let _loading = self.loading.lock().await;
        server.request("thread/unarchive", json!({"threadId":thread})).await?;
        Ok(())
    }
}
