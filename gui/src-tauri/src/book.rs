//! The activity book of a machine (`world-layers.md`, 活动账本), on desktop
//! and devices without a node.

use crate::Shell;
use fastctx_types::access::Written;
use fastctx_types::activity_book::{BookAnswer, BookQuery};

#[fastctx_gui_command::command]
pub(crate) async fn book(
    shell: tauri::State<'_, Shell>,
    machine: Option<String>,
    query: BookQuery,
) -> Result<BookAnswer, String> {
    let machine = machine.map(|machine| machine.parse::<Written>()).transpose().map_err(|error| error.to_string())?;
    shell.world().book(machine, query).await.map_err(|error| error.to_string())
}
