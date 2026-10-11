//! Files fetched for external applications remain in this device's cache.

use fastctx_client::World;
use fastctx_types::MachineName;
use fastctx_types::file_read::{FileRange, ReadFileRequest};
use std::path::Path;
use tokio::io::AsyncWriteExt as _;

pub async fn fetch(world: &World, machine: Option<MachineName>, path: String, cache: &Path) -> Result<String, String> {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .ok_or_else(|| "the file path has no filename".to_string())?
        .to_owned();
    let mut reading = world
        .read_file(machine, ReadFileRequest { path, range: FileRange::All })
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::create_dir_all(cache).await.map_err(|error| error.to_string())?;
    let directory =
        tempfile::Builder::new().prefix("world-file-").tempdir_in(cache).map_err(|error| error.to_string())?;
    let local = directory.path().join(name);
    if local.parent() != Some(directory.path())
        || cfg!(windows) && local.file_name().is_some_and(|name| name.to_string_lossy().contains(':'))
    {
        return Err("the original filename cannot be used as a cache filename on this device".into());
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&local)
        .await
        .map_err(|error| error.to_string())?;
    while let Some(bytes) = reading.next().await.map_err(|error| error.to_string())? {
        file.write_all(&bytes).await.map_err(|error| error.to_string())?;
    }
    file.flush().await.map_err(|error| error.to_string())?;
    drop(file);
    // The external app may read after this command returns. Failed transfers
    // instead drop the private directory, including their provisional bytes.
    let _ = directory.keep();
    Ok(local.to_string_lossy().into_owned())
}
