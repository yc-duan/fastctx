//! Files fetched for external applications remain in this device's cache.

use fastctx_client::file_read::Read;
use fastctx_client::{Reading, World};
use fastctx_types::access::Written;
use fastctx_types::file_read::{FileRange, FileReadInfo, ReadFileRequest};
use std::path::{Path, PathBuf};
use tauri::ipc::Channel;
use tokio::io::AsyncWriteExt as _;

use crate::held_reads::{ReadProgress, tell};

pub async fn fetch(
    world: &World,
    machine: Option<Written>,
    path: String,
    cache: &Path,
    progress: Option<Channel<ReadProgress>>,
) -> Result<String, String> {
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
    let mut reading = world.read_file(machine, ReadFileRequest { path, range: FileRange::All });
    tell(reading.progress(), progress);
    let named = |_: &FileReadInfo| Ok(name.clone());
    let Some((root, generation)) = keeping else {
        return store(&mut reading, named, cache).await.map_err(|stored| stored.to_string());
    };
    let staged = std::path::PathBuf::from(
        store(&mut reading, named, &root.join(".incoming")).await.map_err(|stored| stored.to_string())?,
    );
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

/// Why a file was not stored: its read failed, or this device could not
/// keep it.
pub(crate) enum Stored {
    Read(fastctx_client::Error),
    Kept(String),
}

impl std::fmt::Display for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => error.fmt(f),
            Self::Kept(message) => f.write_str(message),
        }
    }
}

impl From<fastctx_client::Error> for Stored {
    fn from(error: fastctx_client::Error) -> Self {
        Self::Read(error)
    }
}

impl From<std::io::Error> for Stored {
    fn from(error: std::io::Error) -> Self {
        Self::Kept(error.to_string())
    }
}

/// Writes what `reading` hands over into a private folder of its own under
/// `cache`, named as `name_of` says of the file it opened, and says where.
/// A read that starts over writes the file anew, under the name it now has.
pub(crate) async fn store(
    reading: &mut Reading,
    name_of: impl Fn(&FileReadInfo) -> Result<String, String>,
    cache: &Path,
) -> Result<String, Stored> {
    tokio::fs::create_dir_all(cache).await?;
    let directory = tempfile::Builder::new().prefix("world-file-").tempdir_in(cache)?;
    let mut writing: Option<(PathBuf, tokio::fs::File)> = None;
    while let Some(read) = reading.next().await? {
        match read {
            Read::Opened(info) => {
                let local = directory.path().join(name_of(&info).map_err(Stored::Kept)?);
                if local.parent() != Some(directory.path())
                    || cfg!(windows) && local.file_name().is_some_and(|name| name.to_string_lossy().contains(':'))
                {
                    return Err(Stored::Kept(
                        "the original filename cannot be used as a cache filename on this device".into(),
                    ));
                }
                if let Some((before, file)) = writing.take() {
                    drop(file);
                    tokio::fs::remove_file(&before).await?;
                }
                let file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&local).await?;
                writing = Some((local, file));
            }
            Read::Bytes(bytes) => {
                let (_, file) = writing.as_mut().expect("a read hands out bytes only once it opened");
                file.write_all(&bytes).await?;
            }
        }
    }
    let (local, mut file) = writing.expect("a read that ended well opened");
    file.flush().await?;
    drop(file);
    // The external app may read after this command returns. Failed transfers
    // instead drop the private directory, including their provisional bytes.
    let _ = directory.keep();
    Ok(local.to_string_lossy().into_owned())
}
