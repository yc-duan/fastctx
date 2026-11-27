//! Recordings of what a conversation's page was given, kept on this computer
//! when the page found itself showing what its records do not
//! (`src/conversation/self-check.ts`): the newest few, beside the
//! application's data, for the conversation bench to replay. Nothing of them
//! leaves this computer.

use tauri::Manager;

/// How many recordings are kept, and the most one may hold.
const KEPT: usize = 20;
const BYTES_AT_MOST: usize = 4 * 1024 * 1024;

/// Keeps `text` as the recording `name`, the oldest let go past `KEPT`.
#[tauri::command]
pub(crate) async fn conversation_check_keep(app: tauri::AppHandle, name: String, text: String) -> Result<(), String> {
    let named = !name.is_empty()
        && name.len() <= 160
        && name.ends_with(".jsonl")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !named {
        return Err("a recording is named by letters, digits, '-', '_' and '.', ending in .jsonl".into());
    }
    if text.len() > BYTES_AT_MOST {
        return Err(format!("a recording holds at most {BYTES_AT_MOST} bytes"));
    }
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("locating application data: {error}"))?
        .join("conversation-checks");
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        std::fs::create_dir_all(&directory).map_err(|error| format!("creating {}: {error}", directory.display()))?;
        std::fs::write(directory.join(&name), text).map_err(|error| format!("keeping {name}: {error}"))?;
        // Named from when they were made: the first in order are the oldest.
        let mut kept: Vec<std::path::PathBuf> = std::fs::read_dir(&directory)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
            .collect();
        kept.sort();
        let surplus = kept.len().saturating_sub(KEPT);
        for oldest in kept.into_iter().take(surplus) {
            let _ = std::fs::remove_file(oldest);
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}
