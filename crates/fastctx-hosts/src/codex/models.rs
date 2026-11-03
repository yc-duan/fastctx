//! The models a Codex login offers, as its app-server lists them, each with
//! the context window the catalog in its home gives it.

use std::path::Path;

use fastctx_types::agents::{AgentModel, ReasoningEffort};
use serde::Deserialize;
use serde_json::json;

use super::server::AppServer;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    data: Vec<Model>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    id: String,
    model: String,
    display_name: String,
    description: String,
    is_default: bool,
    default_reasoning_effort: String,
    supported_reasoning_efforts: Vec<Effort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Effort {
    reasoning_effort: String,
    description: String,
}

/// Lists the models through `server`, which runs for the Codex home `home`.
pub(super) async fn models(server: &AppServer, home: &Path) -> Result<Vec<AgentModel>, String> {
    let mut models = Vec::new();
    let mut cursor = None::<String>;
    loop {
        let answer = server.request("model/list", json!({ "includeHidden": false, "cursor": cursor })).await?;
        let page: Page = serde_json::from_value(answer).map_err(|error| format!("reading model/list: {error}"))?;
        models.extend(page.data.into_iter().map(|model| {
            AgentModel {
                id: model.id,
                model: model.model,
                display_name: model.display_name,
                description: model.description,
                is_default: model.is_default,
                default_reasoning_effort: model.default_reasoning_effort,
                reasoning_efforts: model
                    .supported_reasoning_efforts
                    .into_iter()
                    .map(|effort| ReasoningEffort { effort: effort.reasoning_effort, description: effort.description })
                    .collect(),
                context_window: None,
                max_context_window: None,
            }
        }));
        cursor = page.next_cursor;
        if cursor.is_none() {
            windows(home, &mut models);
            return Ok(models);
        }
    }
}

/// Each model's context window and the most it may be set to, as the catalog
/// Codex keeps in its home from its service lists them: `model/list` does
/// not say. A catalog it has not written yet leaves them unsaid.
fn windows(home: &Path, models: &mut [AgentModel]) {
    #[derive(Deserialize)]
    struct Catalog {
        models: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        slug: String,
        context_window: Option<u64>,
        max_context_window: Option<u64>,
    }
    let Ok(bytes) = std::fs::read(home.join("models_cache.json")) else { return };
    let Ok(catalog) = serde_json::from_slice::<Catalog>(&bytes) else { return };
    for model in models {
        if let Some(entry) = catalog.models.iter().find(|entry| entry.slug == model.model) {
            model.context_window = entry.context_window.or(entry.max_context_window);
            model.max_context_window = entry.max_context_window.or(entry.context_window);
        }
    }
}
