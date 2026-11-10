//! Tauri commands for the shared conversation window implementation.

mod core;
pub(crate) use core::*;
use std::sync::{Arc, Mutex};

fn shared(shell: &crate::Shell, id: u64) -> Result<Arc<Mutex<Transcript>>, String> {
    shell.transcripts.get(id).ok_or_else(|| "the conversation is no longer watched".to_string())
}

/// Runs `work` on the watched conversation `id` on a thread meant to block:
/// what the page asks of a long conversation never waits on the window's
/// own thread.
async fn on_transcript<R: Send + 'static>(
    shell: &crate::Shell,
    id: u64,
    work: impl FnOnce(&mut Transcript) -> R + Send + 'static,
) -> Result<R, String> {
    let transcript = shared(shell, id)?;
    tauri::async_runtime::spawn_blocking(move || work(&mut transcript.lock().expect("transcript")))
        .await
        .map_err(|error| error.to_string())
}

/// A part of a watched conversation; none when the page asked about an
/// earlier reading or there is nothing there.
#[tauri::command]
pub(crate) async fn conversation_part(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    edge: Edge,
) -> Result<Option<Part>, String> {
    on_transcript(&shell, id, move |transcript| transcript.part(generation, edge)).await
}

#[tauri::command]
pub(crate) async fn conversation_locate(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    turn: String,
) -> Result<Option<usize>, String> {
    on_transcript(&shell, id, move |transcript| transcript.locate(generation, &turn)).await
}

#[tauri::command]
pub(crate) async fn conversation_cut(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    events: usize,
) -> Result<Option<Cut>, String> {
    on_transcript(&shell, id, move |transcript| transcript.cut(generation, events)).await
}

#[tauri::command]
pub(crate) async fn conversation_index(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    start: usize,
    end: usize,
    docs: Vec<Doc>,
) -> Result<bool, String> {
    on_transcript(&shell, id, move |transcript| transcript.index(generation, start, end, docs)).await
}

#[tauri::command]
pub(crate) async fn conversation_find(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    query: String,
    held: Vec<(usize, usize)>,
) -> Result<Option<Found>, String> {
    on_transcript(&shell, id, move |transcript| transcript.find(generation, &query, &held)).await
}
