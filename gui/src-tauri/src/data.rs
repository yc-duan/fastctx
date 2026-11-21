//! The settings page's local data requests, without a network destination.

use fastctx_types::{Call, Reply};
use fastctx_types::data::{CopiesAnswer, CopiesAsk, DataCall, DataReply};

use crate::Shell;

#[cfg(desktop)]
#[fastctx_gui_command::command]
pub(crate) async fn data(shell: tauri::State<'_, Shell>, call: DataCall) -> Result<DataReply, String> {
    shell.connection.local_world()?.data(call).await.map_err(|error| error.to_string())
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
