//! Read the project's map independently of its live World listing.
use fastctx_types::{TownName, project::MapView, town::WorldWho};

#[fastctx_gui_command::command]
pub(crate) async fn project_map(shell: tauri::State<'_, crate::Shell>, town: String) -> Result<MapView, String> {
    let town = TownName::try_from(town).map_err(|error| error.to_string())?;
    shell.world().map(Some(town)).await.map_err(|error| error.to_string())
}

/// Historical actions of residents that left this town are not in WatchWorld.
#[fastctx_gui_command::command]
pub(crate) async fn project_who(shell: tauri::State<'_, crate::Shell>, town: String) -> Result<WorldWho, String> {
    let town = TownName::try_from(town).map_err(|error| error.to_string())?;
    shell.world().who(Some(town), true).await.map_err(|error| error.to_string())
}
