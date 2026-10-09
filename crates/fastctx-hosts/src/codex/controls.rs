//! Stable conversation controls of app-server 0.160. Choices are local to
//! the World's next turn; they do not use the experimental settings RPCs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CodexHost, ThreadSpec};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Selection {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl Selection {
    fn path(root: &Path, thread: &str) -> PathBuf {
        // A host ID is opaque input, never a filesystem path component.
        let key: String = thread.bytes().map(|byte| format!("{byte:02x}")).collect();
        root.join("choices").join(format!("{key}.json"))
    }

    pub(super) fn read(root: &Path, thread: &str) -> Result<Self, String> {
        let path = Self::path(root, thread);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("reading {}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("reading {}: {error}", path.display())),
        }
    }

    fn write(&self, root: &Path, thread: &str) -> Result<(), String> {
        let path = Self::path(root, thread);
        ensure_private_dir(path.parent().expect("choices directory"))
            .and_then(|()| publish(&path, &serde_json::to_vec(self)?, Permissions::Private))
            .map_err(|error| format!("saving the next turn's model: {error}"))
    }

    pub(super) fn apply(&self, params: &mut Value) {
        if let Some(model) = &self.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &self.effort {
            params["effort"] = json!(effort);
        }
    }
}

impl CodexHost {
    pub async fn select_model(
        self: &Arc<Self>,
        thread: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<(), String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        let models = self.models(&server).await?;
        let mut state = self.lock();
        let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
        let selection = Selection {
            model: model.or_else(|| entry.selection.model.clone()).or_else(|| entry.model.clone()),
            effort: effort.or_else(|| entry.selection.effort.clone()).or_else(|| entry.effort.clone()),
        };
        let selected = selection.model.as_deref().ok_or("Codex has not reported this thread's model")?;
        let model = models
            .iter()
            .find(|model| model.id == selected)
            .ok_or_else(|| format!("Codex does not offer model {selected}"))?;
        if let Some(effort) = &selection.effort
            && !model.efforts.contains(effort)
        {
            return Err(format!(
                "model {selected} does not offer reasoning effort {effort}; choose one of: {}",
                model.efforts.join(", ")
            ));
        }
        selection.write(&self.workdir, thread)?;
        entry.selection = selection;
        Ok(())
    }

    pub async fn rename(self: &Arc<Self>, thread: &str, name: &str) -> Result<(), String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        server.request("thread/name/set", json!({"threadId":thread,"name":name})).await?;
        Ok(())
    }

    /// Starts compaction. Completion remains a normal turn/item event.
    pub async fn compact(self: &Arc<Self>, thread: &str) -> Result<(), String> {
        self.prepare(thread).await?;
        if self.running_turn(thread).is_some() {
            return Err("Codex can be compacted only between turns".into());
        }
        let server = self.server().await?;
        server.request("thread/compact/start", json!({"threadId":thread})).await?;
        Ok(())
    }

    /// Forks history with a new resident's instructions and tool identity.
    pub async fn fork(
        self: &Arc<Self>,
        source: &str,
        through: Option<&str>,
        spec: ThreadSpec,
    ) -> Result<(String, Option<String>), String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, source).await?;
        let selection = {
            let mut state = self.lock();
            let entry = state.threads.get(source).ok_or("unknown source thread")?;
            Selection {
                model: entry.selection.model.clone().or_else(|| entry.model.clone()),
                effort: entry.selection.effort.clone().or_else(|| entry.effort.clone()),
            }
        };
        let mut config = spec.config.clone();
        if let Some(effort) = &selection.effort {
            config["model_reasoning_effort"] = json!(effort);
        }
        let answer = server
            .request(
                "thread/fork",
                json!({
                    "threadId": source, "lastTurnId": through, "cwd": spec.cwd,
                    "model": selection.model, "approvalPolicy":"never", "sandbox":"danger-full-access",
                    "developerInstructions":spec.developer_instructions,"config":config,
                    "excludeTurns":true,
                }),
            )
            .await?;
        let thread = answer
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or("thread/fork answered without a thread ID")?
            .to_string();
        let model = answer.get("model").and_then(Value::as_str).map(str::to_string);
        let mut state = self.lock();
        let current = matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, &server));
        let entry = state.threads.entry(thread.clone()).or_default();
        entry.spec = Some(spec);
        entry.loaded = current;
        entry.completed_item = true;
        entry.model = model.clone();
        entry.effort = answer.get("reasoningEffort").and_then(Value::as_str).map(str::to_string);
        entry.metadata = metadata(&answer["thread"]);
        selection
            .write(&self.workdir, &thread)
            .map_err(|error| format!("Codex created fork {thread}, but saving its choices failed: {error}"))?;
        entry.selection = selection;
        Ok((thread, model))
    }

    pub async fn rewind(self: &Arc<Self>, thread: &str, from: &str) -> Result<(), String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        let previous = {
            let mut state = self.lock();
            let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
            let previous = (entry.tools_revision, entry.tools_settled, entry.tools_missing.take(), entry.usage.take());
            // Revert reloads the runtime and starts its MCP servers again.
            // A following turn must wait for that startup, not the old one.
            entry.tools_settled = false;
            previous
        };
        let answer = match server.request("thread/revert", json!({"threadId":thread,"beforeTurnId":from})).await {
            Ok(answer) => answer,
            Err(error) => {
                let mut state = self.lock();
                let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
                if entry.tools_revision == previous.0 {
                    entry.tools_settled = previous.1;
                    entry.tools_missing = previous.2;
                    if entry.usage.is_none() {
                        entry.usage = previous.3;
                    }
                }
                return Err(error);
            }
        };
        let mut state = self.lock();
        let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
        // Turn-end notifications already retire the interrupted runtime.
        // Preserve reports from its replacement, even before this reply.
        entry.metadata = metadata(&answer["thread"]);
        Ok(())
    }

    pub(super) fn observe_controls(&self, method: &str, params: &Value) {
        let resolved = (method == "serverRequest/resolved").then(|| self.request_address(&params["requestId"]));
        let Some(thread) = params.get("threadId").or_else(|| params.pointer("/thread/id")).and_then(Value::as_str)
        else {
            return;
        };
        let mut state = self.lock();
        let entry = state.threads.entry(thread.into()).or_default();
        match method {
            "thread/started" => entry.metadata = metadata(&params["thread"]),
            "thread/tokenUsage/updated" => entry.usage = Some(params["tokenUsage"].clone()),
            "thread/settings/updated" => {
                entry.model = params.pointer("/threadSettings/model").and_then(Value::as_str).map(str::to_string);
                entry.effort = params.pointer("/threadSettings/effort").and_then(Value::as_str).map(str::to_string);
            }
            "serverRequest/resolved" => {
                entry.requests.remove(&resolved.expect("resolved address").to_string());
            }
            "thread/closed" => entry.requests.clear(),
            _ => {}
        }
    }
}

pub(super) fn metadata(thread: &Value) -> Value {
    // Control availability uses these flags; thread replies can also contain full turn history.
    json!({ "historyMode": thread["historyMode"], "ephemeral": thread["ephemeral"] })
}
