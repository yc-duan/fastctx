//! Native agent relationships come from the public app-server protocol.
//! Their results need a loaded parent even after the parent's turn ends.

use super::{CodexHost, State, Thread};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};

pub(super) fn idle(entry: &Thread) -> bool {
    !entry.host_active
        && entry.turn.is_none()
        && !entry.submitted
        && entry.unsettled.is_empty()
        && entry.requests.is_empty()
}

/// Includes the parent, then every observed descendant, once each.
pub(super) fn family(state: &mut State, parent: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![parent.to_owned()];
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(entry) = state.threads.get(&id) {
            pending.extend(entry.children.iter().cloned());
        }
        found.push(id);
    }
    found
}

impl CodexHost {
    /// Rejoins still-loaded native agents before their parent can send more
    /// work. Omitted overrides preserve each agent's own native configuration.
    pub(super) async fn resume_children(&self, server: &Arc<super::AppServer>, parent: &str) -> Result<(), String> {
        let children = {
            let mut state = self.lock();
            family(&mut state, parent)
                .into_iter()
                .skip(1)
                .filter(|id| state.threads.get(id).is_some_and(|entry| entry.loaded && !entry.subscribed))
                .collect::<Vec<_>>()
        };
        for child in children {
            server.request("thread/resume", json!({"threadId":child, "excludeTurns":true})).await?;
            let mut state = self.lock();
            if !matches!(&state.server, Some((current, _)) if Arc::ptr_eq(current, server)) {
                return Err("the app-server changed while subscribing to its agents".into());
            }
            let entry = state.threads.entry(child).or_default();
            entry.loaded = true;
            entry.subscribed = true;
        }
        Ok(())
    }

    pub(super) fn observe_children(&self, method: &str, params: &Value) {
        if !matches!(method, "thread/started" | "thread/status/changed" | "item/started" | "item/completed") {
            return;
        }
        let mut state = self.lock();
        if method == "thread/started" {
            let thread = &params["thread"];
            if let (Some(child), Some(parent)) = (thread["id"].as_str(), thread["parentThreadId"].as_str()) {
                state.threads.entry(parent.into()).or_default().children.insert(child.into());
                let entry = state.threads.entry(child.into()).or_default();
                // Initialized app-server connections are subscribed to native
                // agent threads when the host creates them.
                entry.loaded = true;
                entry.subscribed = true;
            }
        }
        if method == "thread/status/changed"
            && let Some(id) = params["threadId"].as_str()
        {
            let entry = state.threads.entry(id.into()).or_default();
            match params["status"]["type"].as_str() {
                Some("idle" | "notLoaded" | "systemError") => entry.host_active = false,
                _ => entry.host_active = true,
            }
        }
        if matches!(method, "item/started" | "item/completed") {
            let item = &params["item"];
            if item["type"] != "collabAgentToolCall" || item["tool"] != "spawnAgent" {
                return;
            }
            let Some(parent) = params["threadId"].as_str() else { return };
            for child in item["receiverThreadIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                state.threads.entry(parent.into()).or_default().children.insert(child.into());
                let entry = state.threads.entry(child.into()).or_default();
                entry.loaded = true;
                entry.subscribed = true;
                if item["agentsStates"][child]["status"] == "pendingInit"
                    && !entry.completed_item
                    && entry.turn.is_none()
                {
                    entry.host_active = true;
                }
            }
        }
    }
}
