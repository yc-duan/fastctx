//! Moving the hub (`world-network.md`, 把中枢搬到别处): the super
//! administrator's requests go to the node, which asks the hub; the machine
//! the hub moves to has the hub's system service installed with `fastctx
//! hub arrive`, which asks for administrator rights on that machine.

use fastctx_types::hub_move::{HubMoveReply, HubMoveRequest};
use fastctx_types::{AccountName, HubName};

use crate::Shell;

/// `request` of a move of the hub named `hub_name`, or the one hub, as the
/// account `account_name`, its super administrator.
#[fastctx_gui_command::command]
pub(crate) async fn hub_move(
    shell: tauri::State<'_, Shell>,
    hub_name: Option<String>,
    account_name: String,
    request: HubMoveRequest,
) -> Result<HubMoveReply, String> {
    let hub = hub_name.map(HubName::try_from).transpose().map_err(|error| error.to_string())?;
    let account = AccountName::try_from(account_name).map_err(|error| error.to_string())?;
    shell.world().hub_move(hub, account, request).await.map_err(|error| error.to_string())
}

/// Installs the system service of the hub moving to this computer, asking
/// for administrator rights here, and tells the hub: the installed fastctx
/// runs `hub arrive`, as a person would at a terminal.
#[cfg(desktop)]
#[fastctx_gui_command::command]
pub(crate) async fn arrive_hub(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    let program = shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = std::process::Command::new(program);
        command
            .args(["hub", "arrive"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x08000000);
        }
        let output = command.output().map_err(|error| error.to_string())?;
        if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
    })
    .await
    .map_err(|error| error.to_string())?
}
