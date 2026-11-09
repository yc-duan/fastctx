//! History opt-in, shared by the desktop and mobile settings page.

use crate::Shell;
use fastctx_types::{AccountName, HubName, history_switch::HistorySwitch};

fn named(hub: Option<String>, account: Option<String>) -> Result<(Option<HubName>, Option<AccountName>), String> {
    let hub = hub.map(HubName::try_from).transpose().map_err(|error| error.to_string())?;
    let account = account.map(AccountName::try_from).transpose().map_err(|error| error.to_string())?;
    Ok((hub, account))
}

#[fastctx_gui_command::command]
pub(crate) async fn history_switch_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<HistorySwitch, String> {
    let (hub, account) = named(hub, account)?;
    shell.world().history_switch_read(hub, account).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn history_switch_write(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    on: bool,
) -> Result<(), String> {
    let (hub, account) = named(hub, account)?;
    shell.world().history_switch_write(hub, account, on).await.map_err(|error| error.to_string())
}
