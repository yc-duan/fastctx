//! Stable conversation controls of app-server 0.160. Choices are local to
//! the World's next turn; they do not use the experimental settings RPCs.

use std::path::Path;
use std::sync::Arc;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::control_error::ControlError;
use super::{CodexHost, ThreadSpec};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Selection {
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
}

impl Selection {
    pub(super) fn read(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("reading {}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("reading {}: {error}", path.display())),
        }
    }

    pub(super) fn write(&self, path: &Path) -> Result<(), String> {
        ensure_private_dir(path.parent().expect("choices directory"))
            .and_then(|()| publish(path, &serde_json::to_vec(self)?, Permissions::Private))
            .map_err(|error| format!("saving the next turn's model: {error}"))
    }

    pub(super) fn apply(&self, params: &mut Value) {
        if let Some(model) = &self.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &self.effort {
            params["effort"] = json!(effort);
        }
        if let Some(tier) = &self.service_tier {
            params["serviceTier"] = json!(tier);
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
        // A choice is only kept for the next turn: the thread is not loaded
        // and nothing is listed for it, so nothing is asked of Codex now.
        let models = self.catalog();
        let mut state = self.lock();
        let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
        let choices = entry.spec.as_ref().ok_or("how the Codex thread was started is not known")?.choices.clone();
        // A thread not loaded since its app-server started knows its earlier
        // choices only from where they were kept.
        let kept = if entry.loaded { entry.selection.clone() } else { Selection::read(&choices)? };
        let selection = Selection {
            model: model
                .or_else(|| kept.model.clone())
                .or_else(|| entry.model.clone())
                .or_else(|| entry.spec.as_ref().and_then(|spec| spec.started("model"))),
            effort: effort.or_else(|| kept.effort.clone()).or_else(|| entry.effort.clone()),
            service_tier: kept.service_tier.clone().or_else(|| entry.service_tier.clone()),
        };
        let selected = selection.model.as_deref().ok_or("Codex has not reported this thread's model")?;
        if let Some(models) = &models {
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
            if let Some(tier) = &selection.service_tier
                && tier != "default"
                && !model.service_tiers.iter().any(|offer| &offer.id == tier)
            {
                return Err(format!(
                    "model {selected} does not offer service tier {tier}; select /tier default before changing models"
                ));
            }
        }
        selection.write(&choices)?;
        entry.selection = selection;
        Ok(())
    }

    pub async fn rename(self: &Arc<Self>, thread: &str, name: &str) -> Result<(), ControlError> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        server.ask("thread/name/set", json!({"threadId":thread,"name":name})).await?;
        Ok(())
    }

    /// Starts compaction. Completion remains a normal turn/item event.
    pub async fn compact(self: &Arc<Self>, thread: &str) -> Result<(), ControlError> {
        self.prepare(thread).await?;
        if self.running_turn(thread).is_some() {
            return Err("Codex can be compacted only between turns".into());
        }
        self.start_control(thread, "thread/compact/start", json!({"threadId":thread})).await?;
        Ok(())
    }

    /// Forks history with a new resident's instructions and tool identity.
    pub async fn fork(
        self: &Arc<Self>,
        source: &str,
        through: Option<&str>,
        spec: ThreadSpec,
    ) -> Result<(String, Option<String>), ControlError> {
        let server = self.server().await?;
        self.ensure_loaded(&server, source).await?;
        let selection = {
            let mut state = self.lock();
            let entry = state.threads.get(source).ok_or("unknown source thread")?;
            Selection {
                model: entry.selection.model.clone().or_else(|| entry.model.clone()),
                effort: entry.selection.effort.clone().or_else(|| entry.effort.clone()),
                service_tier: entry.selection.service_tier.clone().or_else(|| entry.service_tier.clone()),
            }
        };
        let effective = super::user_config::effective(&server, &spec, self.assisted.is_some()).await?;
        let mut config = effective.config;
        if let Some(effort) = &selection.effort {
            config["model_reasoning_effort"] = json!(effort);
        }
        let answer = server
            .ask(
                "thread/fork",
                json!({
                    "threadId": source, "lastTurnId": through, "cwd": spec.cwd,
                    "model": selection.model, "approvalPolicy":"never", "sandbox":"danger-full-access",
                    "developerInstructions":effective.developer_instructions,"config":config,
                    "excludeTurns":true,
                    "serviceTier":selection.service_tier,
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
        let choices = spec.choices.clone();
        entry.spec = Some(spec);
        entry.loaded = current;
        entry.subscribed = current;
        entry.completed_item = true;
        entry.model = model.clone();
        entry.effort = answer.get("reasoningEffort").and_then(Value::as_str).map(str::to_string);
        entry.service_tier = answer.get("serviceTier").and_then(Value::as_str).map(str::to_string);
        entry.metadata = metadata(&answer["thread"]);
        selection
            .write(&choices)
            .map_err(|error| format!("Codex created fork {thread}, but saving its choices failed: {error}"))?;
        entry.selection = selection;
        Ok((thread, model))
    }

    pub async fn rewind(self: &Arc<Self>, thread: &str, from: &str) -> Result<(), ControlError> {
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
        let answer = match server.ask("thread/revert", json!({"threadId":thread,"beforeTurnId":from})).await {
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
                return Err(error.into());
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
            "model/rerouted" => {
                if let (Some(turn), Some(model)) = (params["turnId"].as_str(), params["toModel"].as_str()) {
                    entry.accounting.rerouted(turn, model);
                    entry.model = Some(model.to_string());
                }
            }
            "thread/reverted" => entry.accounting.reloaded(),
            "thread/started" => {
                entry.metadata = metadata(&params["thread"]);
                // A thread Codex spawned for another names its model and
                // effort only here; one started here already has them.
                let text = |key: &str| params["thread"][key].as_str().map(str::to_string);
                if entry.model.is_none() {
                    entry.model = text("model");
                    entry.effort = text("reasoningEffort");
                }
            }
            "thread/tokenUsage/updated" => entry.usage = Some(params["tokenUsage"].clone()),
            "thread/settings/updated" => {
                entry.model = params.pointer("/threadSettings/model").and_then(Value::as_str).map(str::to_string);
                entry.effort = params.pointer("/threadSettings/effort").and_then(Value::as_str).map(str::to_string);
                entry.service_tier =
                    params.pointer("/threadSettings/serviceTier").and_then(Value::as_str).map(str::to_string);
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
