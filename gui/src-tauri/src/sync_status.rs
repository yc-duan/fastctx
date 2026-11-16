//! How the machines of an account stand in syncing with a hub, for the sync
//! status page (`Call::SyncStatus`, design §28.9): what this computer sees
//! itself, and the hub's reckoning when it is reached.

use fastctx_types::access::HubId;
use fastctx_types::sync_status::{LocalSync, SyncStatus};
use fastctx_types::{AccountName, Call, Reply};
use serde::Serialize;

use crate::Shell;

#[derive(Serialize)]
pub(crate) struct SyncStatusReply {
    local: LocalSync,
    #[serde(skip_serializing_if = "Option::is_none")]
    hub: Option<SyncStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unreached: Option<String>,
}

#[fastctx_gui_command::command]
pub(crate) async fn sync_status(
    shell: tauri::State<'_, Shell>,
    hub: HubId,
    account: AccountName,
    hours: u32,
    machine: Option<fastctx_types::MachineName>,
) -> Result<SyncStatusReply, String> {
    // A computer asks its node; a phone, the hub over its own connection.
    #[cfg(desktop)]
    let world = shell.connection.local_world()?;
    #[cfg(mobile)]
    let world = shell.connection.world();
    match world.call(Call::SyncStatus { hub, account, hours, machine }).await.map_err(|error| error.to_string())?.0 {
        Reply::SyncStatus { local, hub, unreached } => Ok(SyncStatusReply { local, hub: hub.map(|status| *status), unreached }),
        other => Err(format!("the node answered something other than how syncing stands: {other:?}")),
    }
}
