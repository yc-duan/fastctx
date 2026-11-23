//! The settings page's local data requests, without a network destination,
//! on a computer; and what a machine's owner asks of its copies, from any
//! device.

use fastctx_types::data::{CopiesAnswer, CopiesAsk};
#[cfg(desktop)]
use fastctx_types::data::{DataCall, DataReply};
use fastctx_types::{Call, Reply};

use crate::Shell;

#[cfg(desktop)]
#[fastctx_gui_command::command]
pub(crate) async fn data(shell: tauri::State<'_, Shell>, call: DataCall) -> Result<DataReply, String> {
    shell.connection.local_world()?.data(call).await.map_err(|error| error.to_string())
}

/// The rule `machine_name`, this computer or another machine of the network,
/// deletes its conversations idle long enough by: read, or set by its owner
/// or by an account its agents assist; asked as `account` where it is known.
#[fastctx_gui_command::command]
pub(crate) async fn conversation_retention(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    ask: fastctx_types::conversation_retention::RetentionAsk,
    account: Option<fastctx_types::access::HubAccount>,
) -> Result<fastctx_types::conversation_retention::RetentionAnswer, String> {
    let machine = fastctx_types::access::Written::try_from(machine_name).map_err(|error| error.to_string())?;
    match shell
        .world()
        .call(Call::ConversationRetention { machine, ask, account })
        .await
        .map_err(|error| error.to_string())?
        .0
    {
        Reply::ConversationRetention(answer) => Ok(answer),
        other => Err(format!("the World answered {other:?}")),
    }
}

/// What the owner of `machine_name`, this computer or another machine of the
/// network, asks of the copies it holds of the conversations living
/// elsewhere (the machine page's 会话副本).
#[fastctx_gui_command::command]
pub(crate) async fn machine_copies(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    ask: CopiesAsk,
) -> Result<CopiesAnswer, String> {
    let machine = fastctx_types::access::Written::try_from(machine_name).map_err(|error| error.to_string())?;
    match shell.world().call(Call::MachineCopies { machine, ask }).await.map_err(|error| error.to_string())?.0 {
        Reply::MachineCopies(answer) => Ok(answer),
        other => Err(format!("the World answered {other:?}")),
    }
}
