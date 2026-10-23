//! Sentinel account settings; secret values are never returned to the interface.
use crate::Shell;
use fastctx_types::{
    AccountName, HubName,
    sentinel_settings::{SettingsReply, SettingsView, WriteSettings},
};
fn names(hub: Option<String>, account: Option<String>) -> Result<(Option<HubName>, Option<AccountName>), String> {
    Ok((
        hub.map(HubName::try_from).transpose().map_err(|e| e.to_string())?,
        account.map(AccountName::try_from).transpose().map_err(|e| e.to_string())?,
    ))
}
#[fastctx_gui_command::command]
pub(crate) async fn sentinel_settings_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<SettingsView, String> {
    let (hub, account) = names(hub, account)?;
    match shell.world().sentinel_settings(hub, account, None).await.map_err(|e| e.to_string())? {
        SettingsReply::View { view } => Ok(view),
        _ => Err("unexpected settings conflict on read".into()),
    }
}
#[fastctx_gui_command::command]
pub(crate) async fn sentinel_settings_write(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    change: WriteSettings,
) -> Result<SettingsReply, String> {
    let (hub, account) = names(hub, account)?;
    shell.world().sentinel_settings(hub, account, Some(change)).await.map_err(|e| e.to_string())
}
