//! Commands implemented with stable app-server methods. Goal mutations carry
//! the user's provenance (0.162 alpha); 0.160 ignores that additional field.
//! Contracts: app-server-protocol/protocol/v2/{thread,turn,model,config}.rs
//! at rust-v0.160.0, a956835d020762cb2b570053af06f643a11c0ecc.

use std::sync::Arc;

use serde_json::{Value, json};

use super::CodexHost;

impl CodexHost {
    pub async fn goal(self: &Arc<Self>, thread: &str, args: &str) -> Result<Value, String> {
        let args = args.trim();
        let (method, params) = match args {
            "" | "get" => ("thread/goal/get", json!({"threadId":thread})),
            "clear" => ("thread/goal/clear", json!({"threadId":thread,"origin":"user"})),
            _ => {
                let objective = args.strip_prefix("set ").unwrap_or(args).trim();
                if objective.is_empty() || objective == "set" {
                    return Err("/goal set takes an objective".into());
                }
                ("thread/goal/set", json!({"threadId":thread,"objective":objective,"status":"active","origin":"user"}))
            }
        };
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        server.request(method, params).await
    }

    pub async fn read_config(self: &Arc<Self>, thread: &str) -> Result<Value, String> {
        let cwd = self
            .lock()
            .threads
            .get(thread)
            .and_then(|entry| entry.spec.as_ref())
            .map(|spec| spec.cwd.clone())
            .ok_or("this thread's working directory is not known")?;
        self.server().await?.request("config/read", json!({"cwd":cwd,"includeLayers":true})).await
    }

    /// Select a tier ID, or toggle a catalog tier by its slash name. Settings
    /// are saved only after validating the model currently selected for World.
    pub async fn select_tier(self: &Arc<Self>, thread: &str, id: &str, toggle: bool) -> Result<String, String> {
        let server = self.server().await?;
        self.ensure_loaded(&server, thread).await?;
        if id != "default" && self.read_config(thread).await?["config"]["features"]["fast_mode"] == false {
            return Err("Codex has fast_mode disabled; it would ignore this service tier".into());
        }
        let models = self.models(&server).await?;
        let mut state = self.lock();
        let entry = state.threads.get_mut(thread).ok_or("unknown Codex thread")?;
        let model = entry
            .selection
            .model
            .as_ref()
            .or(entry.model.as_ref())
            .ok_or("Codex has not reported this thread's model")?;
        let model =
            models.iter().find(|offer| &offer.id == model).ok_or("the selected model is absent from the catalog")?;
        if id != "default" && !model.service_tiers.iter().any(|tier| tier.id == id) {
            return Err(format!("model {} does not offer service tier {id}", model.id));
        }
        // Catalog defaults are presentation metadata, not a report that the
        // tier is active. Core 0.160's get_service_tier(None, ...) keeps None.
        let current = entry.selection.service_tier.as_ref().or(entry.service_tier.as_ref());
        let selected = if toggle && current.is_some_and(|current| current == id) { "default" } else { id };
        let mut selection = entry.selection.clone();
        selection.service_tier = Some(selected.into());
        selection.write(&self.workdir, thread)?;
        entry.selection = selection;
        Ok(selected.into())
    }
}
