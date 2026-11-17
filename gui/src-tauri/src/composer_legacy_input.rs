//! Replays retained native text through the older request/reply sync boundary
//! before an interface falls back to identity edits. Sends are never translated.
use fastctx_client::World;
use fastctx_types::composer::{Request, Updated, sync};

#[fastctx_gui_command::command]
pub async fn composer_sync_text(
    shell: tauri::State<'_, crate::Shell>,
    resident_id: String,
    machine_name: Option<String>,
    input: sync::Input,
) -> Result<Updated, crate::NotSaid> {
    let refused = |message: String| crate::NotSaid { message, again: false };
    let sync::Input::Update { document, update, .. } = &input else {
        return Err(refused("only text updates can cross the older synchronization boundary".into()));
    };
    if document.len() > 256 || update.0.len() > sync::MAX_UPDATE_BYTES {
        return Err(refused(fastctx_types::composer::DOCUMENT_TOO_LONG.into()));
    }
    shell
        .world()
        .composer(
            crate::resident(resident_id).map_err(refused)?,
            crate::machine(machine_name).map_err(refused)?,
            Request::Sync { input: Box::new(input) },
        )
        .await
        .map_err(|error| crate::NotSaid {
            again: World::unanswered(&error) || World::busy(&error),
            message: error.to_string(),
        })
}
