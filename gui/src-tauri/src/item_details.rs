//! The host's original record is fetched only for the item the reader asks for.

use fastctx_types::{ResidentId, access::Written};

#[derive(serde::Deserialize)]
pub struct Destination {
    machine: Written,
    resident: ResidentId,
}

#[fastctx_gui_command::command]
pub async fn item_detail(
    shell: tauri::State<'_, crate::Shell>,
    address: Destination,
    turn: Option<String>,
    item: String,
) -> Result<Option<serde_json::Value>, String> {
    shell.world().item_detail(address.resident, address.machine, turn, item).await.map_err(|error| error.to_string())
}

/// What one of the host's own agents in a conversation did, read from its
/// own record when the reader opens it.
#[fastctx_gui_command::command]
pub async fn agent_activity(
    shell: tauri::State<'_, crate::Shell>,
    address: Destination,
    agent: String,
) -> Result<fastctx_types::session::AgentActivity, String> {
    shell.world().agent_activity(address.resident, address.machine, agent).await.map_err(|error| error.to_string())
}
