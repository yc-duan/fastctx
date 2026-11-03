//! Legacy acceptance receipts use the same bounded, demand-promoted workers as
//! conversation records. An unreadable receipt never stalls another identity.
use super::delivered;
use crate::{
    Node,
    record_migration::{archive, scheduler},
};
use fastctx_platform::{disk, task};
use fastctx_storage::{Database, receipts};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) fn prepare(database: &Database, path: &Path) -> io::Result<()> {
    let world = path.parent().and_then(Path::parent).ok_or_else(|| io::Error::other("letter receipt has no world"))?;
    let name =
        path.file_name().and_then(|name| name.to_str()).ok_or_else(|| io::Error::other("invalid receipt identity"))?;
    let (database, path, root) = (database.clone(), path.to_path_buf(), world.to_path_buf());
    scheduler::request(world, "@node", &format!("letter-receipt/{name}"), move || import(&database, &root, &path))
}
fn import(database: &Database, world: &Path, path: &Path) -> io::Result<()> {
    let original = match disk::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return archive::damaged(world, "@node", "letter-receipts", path, &error.to_string()),
    };
    if original != b"accepted" {
        return archive::damaged(world, "@node", "letter-receipts", path, "receipt is not an accepted handoff");
    }
    let signature = archive::fingerprint(path)?;
    let key = delivered::key(path.file_name().and_then(|name| name.to_str()).expect("receipt identity"));
    let imported = key.clone();
    database
        .write_now_at(scheduler::priority(), key.bytes() + 8, move |db| {
            receipts::import(db, &imported, b"accepted", delivered::KEEP, jiff::Timestamp::now().as_millisecond())
        })
        .map_err(io::Error::other)?;
    let reread = disk::read(path)?;
    let stored = database.read_background_now(|db| receipts::get(db, &key)).map_err(io::Error::other)?;
    if reread != original || stored.as_deref() != Some(reread.as_slice()) {
        return archive::damaged(world, "@node", "letter-receipts", path, "receipt differs from reread original");
    }
    archive::verified(world, "@node", path, &signature)
}
fn all(database: &Database, world: &Path) -> io::Result<()> {
    let entries = match disk::read_dir(world.join("letter-receipts")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        scheduler::checkpoint()?;
        let entry = entry?;
        if entry.file_type()?.is_file() && !entry.file_name().to_string_lossy().starts_with('.') {
            prepare(database, &entry.path())?;
        }
    }
    Ok(())
}
pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    task::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let working = node.clone();
            let result = task::spawn_blocking(move || {
                let world: PathBuf = working.world.path().into();
                let source = world.clone();
                let database = working.database.clone();
                scheduler::discovery(|| {
                    scheduler::request(&world, "@node", "letter-receipts", move || all(&database, &source))
                })
            })
            .await;
            match result {
                Ok(Ok(())) => return,
                Ok(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                Ok(Err(error)) => node.log.line(format_args!("moving business letter receipts: {error}")),
                Err(error) => node.log.line(format_args!("letter receipt migration worker: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}
