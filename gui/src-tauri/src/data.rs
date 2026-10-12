//! The settings page's local data requests, without a network destination.

use fastctx_types::data::{DataCall, DataReply};

use crate::Shell;

#[cfg(desktop)]
#[fastctx_gui_command::command]
pub(crate) async fn data(shell: tauri::State<'_, Shell>, call: DataCall) -> Result<DataReply, String> {
    shell.connection.local_world()?.data(call).await.map_err(|error| error.to_string())
}
