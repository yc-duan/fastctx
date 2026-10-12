//! The host's original record is fetched only for the item the reader asks for.

use fastctx_types::Address;

#[fastctx_gui_command::command]
pub async fn item_detail(
    shell: tauri::State<'_, crate::Shell>,
    address: Address,
    turn: Option<String>,
    item: String,
) -> Result<Option<serde_json::Value>, String> {
    shell.world().item_detail(address, turn, item).await.map_err(|error| error.to_string())
}
