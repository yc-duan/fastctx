//! Account configuration commands, shared by desktop and Android.

use crate::Shell;
use fastctx_types::mcp::{McpCall, McpReply, ProfileView};
use fastctx_types::profile::Profile;
use fastctx_types::{AccountName, HubName};

fn named(hub: Option<String>, account: Option<String>) -> Result<(Option<HubName>, Option<AccountName>), String> {
    Ok((
        hub.map(HubName::try_from).transpose().map_err(|e| e.to_string())?,
        account.map(AccountName::try_from).transpose().map_err(|e| e.to_string())?,
    ))
}

#[fastctx_gui_command::command]
pub(crate) async fn profile_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<ProfileView, String> {
    let (hub, account) = named(hub, account)?;
    shell.world().profile_read(hub, account).await.map_err(|e| e.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn profile_write(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    profile: Profile,
) -> Result<u64, String> {
    let (hub, account) = named(hub, account)?;
    shell.world().profile_write(hub, account, profile).await.map_err(|e| e.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn mcp(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    mcp: McpCall,
) -> Result<McpReply, String> {
    let (hub, account) = named(hub, account)?;
    shell.world().mcp(hub, account, mcp).await.map_err(|e| e.to_string())
}
