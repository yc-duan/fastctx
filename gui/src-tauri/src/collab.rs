//! The account's model collaboration settings, shared by desktop and Android.

use fastctx_types::collab::{Collab, CollabView};
use fastctx_types::{AccountName, HubName};

use crate::Shell;

#[fastctx_gui_command::command]
pub(crate) async fn collab_read(
    shell: tauri::State<'_, Shell>,
    hub: String,
    account: String,
) -> Result<CollabView, String> {
    shell
        .world()
        .collab_read(
            HubName::try_from(hub).map_err(|error| error.to_string())?,
            AccountName::try_from(account).map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn collab_write(
    shell: tauri::State<'_, Shell>,
    hub: String,
    account: String,
    collab: Collab,
) -> Result<u64, String> {
    shell
        .world()
        .collab_write(
            HubName::try_from(hub).map_err(|error| error.to_string())?,
            AccountName::try_from(account).map_err(|error| error.to_string())?,
            collab,
        )
        .await
        .map_err(|error| error.to_string())
}
