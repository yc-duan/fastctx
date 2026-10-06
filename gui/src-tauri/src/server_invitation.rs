//! The server's installer line comes from the local node, not the GUI.

use crate::Shell;

#[tauri::command]
pub(crate) async fn invite_server(
    shell: tauri::State<'_, Shell>,
    name: Option<String>,
    hours: u32,
    scope: Option<fastctx_types::scope::NamedScope>,
) -> Result<String, String> {
    let expires = jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_hours(i64::from(hours)))
        .map_err(|error| error.to_string())?;
    shell.world().invite_server(crate::machine(name)?, expires, scope).await.map_err(|error| error.to_string())
}
