//! Stable lifecycle RPCs. Retiring a subscription and unloading a thread
//! are different facts: other clients and Codex's idle delay still apply.

use super::CodexHost;
use serde_json::json;
use std::sync::Arc;

impl CodexHost {
    /// Subscription can be released only after the node has acknowledged
    /// every terminal report and no host interaction remains answerable.
    pub fn releasable(&self, thread: &str) -> bool {
        let mut state = self.lock();
        state.threads.get(thread).is_some_and(|entry| entry.subscribed)
            && super::children::family(&mut state, thread)
                .iter()
                .all(|id| state.threads.get(id).is_some_and(super::children::idle))
    }

    pub fn subscribed(&self, thread: &str) -> bool {
        self.lock().threads.get(thread).is_some_and(|entry| entry.subscribed)
    }

    pub fn loaded(&self, thread: &str) -> bool {
        self.lock().threads.get(thread).is_some_and(|entry| entry.loaded)
    }

    /// Returns true only once unloading is observed. False means this
    /// connection unsubscribed but Codex still holds the runtime.
    pub async fn sleep(self: &Arc<Self>, thread: &str) -> Result<bool, String> {
        let server = self.server().await?;
        let _loading = self.loading.lock().await;
        let family = {
            let mut state = self.lock();
            let family = super::children::family(&mut state, thread);
            if family.iter().any(|id| state.threads.get(id).is_some_and(|entry| !super::children::idle(entry))) {
                return Err("this Codex thread or one of its agents still has work and cannot sleep".into());
            }
            family
        };
        let mut absent = false;
        // Idle children are automatic subscriptions too. Let those go first;
        // the parent stays available if any native operation fails.
        for id in family.into_iter().rev() {
            let answer = server.request("thread/unsubscribe", json!({"threadId":id})).await?;
            let status = answer.get("status").and_then(serde_json::Value::as_str);
            if !matches!(status, Some("unsubscribed" | "notSubscribed" | "notLoaded")) {
                return Err(format!("thread/unsubscribe returned an unknown result: {answer}"));
            }
            let mut state = self.lock();
            if let Some(entry) = state.threads.get_mut(&id) {
                entry.subscribed = false;
                if status == Some("notLoaded") {
                    entry.loaded = false;
                }
                if id == thread {
                    absent = !entry.loaded;
                }
            } else if id == thread {
                absent = status == Some("notLoaded");
            }
        }
        Ok(absent)
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
        // Codex says it does not know the thread in either of these ways:
        // 0.160 names the thread, later versions the rollout it lacks.
        let deleted = match server.ask("thread/delete", json!({"threadId":thread})).await {
            Ok(_) => true,
            Err(super::server::Unanswered::Rpc { code: -32600, message, .. })
                if message == format!("thread not found: {thread}")
                    || message == format!("no rollout found for thread id {thread}") =>
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
