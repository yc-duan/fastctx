//! The account's layer (`world-layers.md`), on desktop and devices without a node.

use crate::Shell;
use fastctx_types::layers::{Chosen, Layer};
use fastctx_types::{AccountName, HubName, ResidentId};

fn scope(hub: Option<String>, account: Option<String>) -> Result<(Option<HubName>, Option<AccountName>), String> {
    Ok((
        hub.map(HubName::try_from).transpose().map_err(|error| error.to_string())?,
        account.map(AccountName::try_from).transpose().map_err(|error| error.to_string())?,
    ))
}

#[fastctx_gui_command::command]
pub(crate) async fn layer_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<Chosen, String> {
    let (hub, account) = scope(hub, account)?;
    shell.world().layer(hub, account, None).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn layer_set(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    layer: Layer,
) -> Result<Chosen, String> {
    let (hub, account) = scope(hub, account)?;
    shell.world().layer(hub, account, Some(layer)).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn bring_in(shell: tauri::State<'_, Shell>, resident: String) -> Result<(), String> {
    let resident = ResidentId::try_from(resident).map_err(|error| error.to_string())?;
    shell.world().bring_in(resident).await.map_err(|error| error.to_string())
}
