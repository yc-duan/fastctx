//! Bringing a session the user opened on their own into Fastctx
//! (`world-layers.md`, 接进来).

use crate::Shell;
use fastctx_types::ResidentId;

#[fastctx_gui_command::command]
pub(crate) async fn bring_in(shell: tauri::State<'_, Shell>, resident: String) -> Result<(), String> {
    let resident = ResidentId::try_from(resident).map_err(|error| error.to_string())?;
    shell.world().bring_in(resident).await.map_err(|error| error.to_string())
}
