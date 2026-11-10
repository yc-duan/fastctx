//! Native command for explicitly expanding one oversized timeline event.

#[fastctx_gui_command::command]
pub(crate) async fn conversation_object(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    at: usize,
) -> Result<Box<serde_json::value::RawValue>, String> {
    let reader = shell.timeline_readers.get(id).ok_or("the conversation is no longer watched")?;
    crate::timeline_body::read(reader, generation, at).await
}
