//! What the town answers on the node's local admin channel: requests from `fastctx` commands
//! a resident runs on this machine, such as `fastctx project enter`.

use super::core::Town;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum TownRequest {
    /// Moves a resident into a town and answers with the town's map.
    Enter { resident: String, town: String },
}

pub(crate) async fn handle(town: &Town, request: TownRequest) -> Result<Value, String> {
    match request {
        TownRequest::Enter { resident, town: name } => Ok(json!({ "map": town.enter(&resident, &name)? })),
    }
}
