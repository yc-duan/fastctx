//! Legacy composer receipts remain deduplication evidence. Move them in bounded
//! transactions instead of deleting them merely because their files are old.

use fastctx_storage::{
    Database, Error,
    rusqlite::{OptionalExtension, params},
};
use std::io;
use std::path::{Path, PathBuf};

pub(super) fn migrate(
    database: &Database,
    owner: &str,
    directory: &fastctx_types::layout::ConversationDir,
) -> io::Result<usize> {
    let exists = database
        .read_now(|connection| {
            Ok(connection
                .query_row("SELECT 1 FROM composers WHERE owner=?1", [owner], |_| Ok(()))
                .optional()?
                .is_some())
        })
        .map_err(io::Error::other)?;
    // The draft migration creates the parent row. Leaving files here preserves
    // dual reads if that migration has not yet succeeded.
    if !exists {
        return Ok(0);
    }
    let mut count = 0;
    for kind in ["receipts", "uploads", "ended"] {
        let books = directory.composer_books(kind);
        let entries = match std::fs::read_dir(&books) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let mut batch = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.path().extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let path = entry.path();
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            validate(kind, &bytes).map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
            batch.push((path, bytes));
            if batch.len() == 256 {
                count += commit(database, owner, kind, std::mem::take(&mut batch))?;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        count += commit(database, owner, kind, batch)?;
        match std::fs::remove_dir(&books) {
            Ok(()) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}

fn validate(kind: &str, bytes: &[u8]) -> serde_json::Result<()> {
    match kind {
        "receipts" => serde_json::from_slice::<super::state::Receipt>(bytes).map(|_| ()),
        "uploads" => serde_json::from_slice::<super::uploads::UploadRecord>(bytes).map(|_| ()),
        "ended" => serde_json::from_slice::<bool>(bytes).map(|_| ()),
        _ => unreachable!(),
    }
}
fn name(path: &Path) -> fastctx_storage::Result<&str> {
    path.file_stem().and_then(|name| name.to_str()).ok_or_else(|| Error::new("composer receipt name is not UTF-8"))
}
fn commit(database: &Database, owner: &str, kind: &'static str, batch: Vec<(PathBuf, Vec<u8>)>) -> io::Result<usize> {
    if batch.is_empty() {
        return Ok(0);
    }
    let bytes = batch.iter().map(|(path, bytes)| path.as_os_str().len() + bytes.len()).sum();
    let owner = owner.to_owned();
    let batch = database.write_now(bytes, move |connection| {
        for (path, bytes) in &batch {
            let name = name(path)?;
            connection.execute(
                "INSERT INTO composer_records(owner,kind,name,content,expires) VALUES(?1,?2,?3,?4,NULL) ON CONFLICT DO NOTHING",
                params![owner, kind, name, bytes]
            )?;
            let (stored, expires): (Vec<u8>, Option<i64>) = connection.query_row(
                "SELECT content,expires FROM composer_records WHERE owner=?1 AND kind=?2 AND name=?3",
                params![owner, kind, name], |row| Ok((row.get(0)?,row.get(1)?))
            )?;
            if expires.is_none() && stored != *bytes {
                return Err(Error::new("legacy composer receipt copies disagree; source preserved"));
            }
        }
        Ok(batch)
    }).map_err(io::Error::other)?;
    // The write answer means the FULL transaction committed. A concurrent live
    // operation can supersede the receipt, but does not remove its row here.
    let mut count = 0;
    for (path, _) in batch {
        let _transition = crate::resident_storage::lock();
        match std::fs::remove_file(path) {
            Ok(()) => count += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}
