//! Native destination selection, followed by a bounded copy on this device.

use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "android"))]
use std::path::Path;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Saved,
    Cancelled,
    Failed { reason: String },
}

pub async fn save(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    path: String,
    suggested_name: String,
) -> Outcome {
    match save_inner(app, window, path, suggested_name).await {
        Ok(outcome) => outcome,
        Err(reason) => Outcome::Failed { reason },
    }
}

async fn save_inner(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    path: String,
    suggested_name: String,
) -> Result<Outcome, String> {
    if suggested_name.is_empty()
        || suggested_name.contains(['\0', '/', '\\'])
        || matches!(suggested_name.as_str(), "." | "..")
    {
        return Err("the suggested name must be a single filename".into());
    }
    #[cfg(target_os = "android")]
    {
        let _ = window;
        return crate::android::save_as(&app, path, suggested_name).await;
    }
    #[cfg(not(target_os = "android"))]
    {
        let source = fastctx_platform::local_path(&path).map_err(|error| error.to_string())?;
        if !source.is_absolute() {
            return Err("the source path must be absolute".into());
        }
        // Open before asking: an unreadable source is a failure, and this
        // handle still names the chosen original if its path is replaced.
        let file = fastctx_platform::file_read::open(&source).map_err(|error| error.to_string())?;
        if !file.metadata().map_err(|error| error.to_string())?.is_file() {
            return Err("the source must be an ordinary file".into());
        }
        #[cfg(windows)]
        let destination = {
            let _ = app;
            windows::choose(window, suggested_name).await?
        };
        #[cfg(target_os = "linux")]
        let destination = {
            let _ = (app, window);
            linux::choose(suggested_name).await?
        };
        #[cfg(target_os = "macos")]
        let destination = {
            let _ = window;
            macos::choose(app, suggested_name).await?
        };
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        let destination: Option<std::path::PathBuf> = {
            let _ = (app, window);
            return Err("save as is not supported on this platform".into());
        };
        let Some(destination) = destination else { return Ok(Outcome::Cancelled) };
        tokio::task::spawn_blocking(move || copy(file, &source, &destination))
            .await
            .map_err(|error| error.to_string())??;
        Ok(Outcome::Saved)
    }
}

#[cfg(not(target_os = "android"))]
fn copy(mut source: std::fs::File, source_path: &Path, destination: &Path) -> Result<(), String> {
    use std::io::Write as _;
    let before = fastctx_platform::file_read::snapshot(&source, &source.metadata().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let parent = destination.parent().ok_or("the save destination has no parent folder")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    let bytes = std::io::copy(&mut std::io::Read::take(&mut source, before.size), temporary.as_file_mut())
        .map_err(|error| error.to_string())?;
    temporary.flush().map_err(|error| error.to_string())?;
    let after = fastctx_platform::file_read::snapshot(&source, &source.metadata().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    let current = {
        let file = fastctx_platform::file_read::open(source_path).map_err(|error| error.to_string())?;
        fastctx_platform::file_read::snapshot(&file, &file.metadata().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?
    };
    if before != after || before != current || bytes != before.size {
        return Err("the source changed while saving".into());
    }
    // Commit beside the destination. A failed copy leaves an existing
    // destination intact, including when it aliases the source file.
    drop(source);
    temporary.persist(destination).map_err(|error| error.to_string())?;
    Ok(())
}
