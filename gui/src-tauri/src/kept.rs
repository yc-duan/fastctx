//! What this device keeps of the conversations it viewed (`fastctx_client::cache`),
//! for the settings' 本机数据 → 缓存: how much, letting go of all of it, or
//! of one conversation.

use fastctx_client::cache::Usage;
use fastctx_types::Address;

use crate::Shell;

fn cache(shell: &Shell) -> Result<std::sync::Arc<fastctx_client::cache::Cache>, String> {
    shell.world().cache().cloned().ok_or_else(|| "this interface keeps nothing it views".to_string())
}

/// How much is kept, conversation by conversation, the largest first.
#[fastctx_gui_command::command]
pub async fn kept_usage(shell: tauri::State<'_, Shell>) -> Result<Usage, String> {
    let cache = cache(&shell)?;
    tauri::async_runtime::spawn_blocking(move || cache.usage())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// Lets go of everything kept.
#[fastctx_gui_command::command]
pub async fn kept_clear(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    let cache = cache(&shell)?;
    tauri::async_runtime::spawn_blocking(move || cache.clear())
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

/// Lets go of one conversation and the pictures only it showed.
#[fastctx_gui_command::command]
pub async fn kept_forget(shell: tauri::State<'_, Shell>, address: Address) -> Result<(), String> {
    let cache = cache(&shell)?;
    tauri::async_runtime::spawn_blocking(move || cache.forget(&address))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}
