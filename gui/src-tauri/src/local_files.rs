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
    let keeping = match world.cache() {
        Some(cache) => {
            let root = cache.root().to_path_buf();
            let reading = root.clone();
            let generation = tauri::async_runtime::spawn_blocking(move || {
                fastctx_platform::viewed_cache::Turn::acquire(&reading).map(|turn| turn.generation())
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
            Some((root, generation))
        }
        None => None,
    };
    let reading = world
        .read_file(machine, ReadFileRequest { path, range: FileRange::All })
        .await
        .map_err(|error| error.to_string())?;
    let Some((root, generation)) = keeping else { return store(reading, name, cache).await };
    let staged = std::path::PathBuf::from(store(reading, name, &root.join(".incoming")).await?);
    let folder = staged.parent().expect("a fetched file has its own folder").to_path_buf();
    let pending = Pending(folder.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let _pending = pending;
        let turn = fastctx_platform::viewed_cache::Turn::acquire(&root)?;
        if turn.generation() != generation {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "the viewed cache was cleared during the file read",
            ));
        }
        let files = root.join("files");
        fastctx_platform::ensure_private_dir(&files)?;
        let target = files.join(folder.file_name().expect("a fetched file folder has a name"));
        std::fs::rename(&folder, &target)?;
        Ok::<_, std::io::Error>(target.join(staged.file_name().expect("a fetched file has a name")))
    })
    .await
    .map_err(|error| error.to_string())?
    .map(|path| path.to_string_lossy().into_owned())
    .map_err(|error| error.to_string())
}

struct Pending(std::path::PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("discarding an unfinished file cache: {error}");
        }
    }
}

pub(crate) async fn store(mut reading: fastctx_client::Reading, name: String, cache: &Path) -> Result<String, String> {
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
