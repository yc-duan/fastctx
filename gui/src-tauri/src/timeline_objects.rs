//! Native command for explicitly expanding one oversized timeline event.

#[derive(serde::Serialize)]
pub(crate) struct ExpandedEvent {
    object: String,
    event: Box<serde_json::value::RawValue>,
}

#[fastctx_gui_command::command]
pub(crate) async fn conversation_object(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    object: String,
) -> Result<ExpandedEvent, String> {
    let reader = shell.timeline_readers.get(id).ok_or("the conversation is no longer watched")?;
    let event = crate::timeline_body::read(reader, generation, object.clone()).await?;
    Ok(ExpandedEvent { object, event })
}
