//! Model collaboration settings, shared by desktop and Android: an
//! account's, or, with no account named, this computer's own, which apply
//! while it is in no network.

use fastctx_types::collab::{Collab, CollabView};
use fastctx_types::{AccountName, HubName};

use crate::Shell;

fn named(hub: Option<String>, account: Option<String>) -> Result<(Option<HubName>, Option<AccountName>), String> {
    let hub = hub.map(HubName::try_from).transpose().map_err(|error| error.to_string())?;
    let account = account.map(AccountName::try_from).transpose().map_err(|error| error.to_string())?;
    Ok((hub, account))
}

#[fastctx_gui_command::command]
pub(crate) async fn collab_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<CollabView, String> {
    let (hub, account) = named(hub, account)?;
    shell.world().collab_read(hub, account).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn collab_write(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    collab: Collab,
) -> Result<u64, String> {
    let (hub, account) = named(hub, account)?;
    shell.world().collab_write(hub, account, collab).await.map_err(|error| error.to_string())
}
