//! The conversation side panel's collaboration tree.

use crate::Shell;
use fastctx_types::{ResidentId, access::Written, collaboration::Tree};

#[fastctx_gui_command::command]
pub(crate) async fn collaboration_tree(
    shell: tauri::State<'_, Shell>,
    resident: String,
    machine: Option<String>,
) -> Result<Tree, String> {
    let resident = ResidentId::try_from(resident).map_err(|error| error.to_string())?;
    let machine = machine.map(|machine| Written::parse(&machine)).transpose().map_err(|error| error.to_string())?;
    shell.world().collaboration(resident, machine).await.map_err(|error| error.to_string())
}
