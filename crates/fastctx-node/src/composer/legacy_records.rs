//! Legacy composer receipts remain deduplication evidence. Move them in bounded
//! transactions instead of deleting them merely because their files are old;
//! each original then goes to the archive for its retention, and one that
//! cannot be read is set aside alone with its reason.

use crate::record_migration::archive;
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
    let world = crate::record_migration::world(directory)?;
    let mut count = 0;
    for kind in ["receipts", "uploads", "ended"] {
        let books = directory.composer_books(kind);
        let entries = match fastctx_platform::disk::read_dir(&books) {
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
            // What is checked and kept is what the fingerprint names, so a file
            // changed meanwhile stays for the next sweep.
            let signature = match archive::fingerprint(&path) {
                Ok(signature) => signature,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let bytes = match fastctx_platform::disk::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            // A receipt that cannot be read is set aside with its reason; the
            // others still move.
            if let Err(error) = validate(kind, &bytes) {
                let error = io::Error::new(io::ErrorKind::InvalidData, error);
                crate::record_migration::set_aside(directory, &format!("composer-{kind}"), &path, &error)?;
                continue;
            }
            batch.push((path, bytes, signature));
            if batch.len() == 16 || batch.iter().map(|(_, bytes, _)| bytes.len()).sum::<usize>() >= 128 * 1024 {
                count += commit(database, &world, owner, directory, kind, std::mem::take(&mut batch))?;
            }
        }
        count += commit(database, &world, owner, directory, kind, batch)?;
        match fastctx_platform::disk::remove_dir(&books) {
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
fn commit(
    database: &Database,
    world: &Path,
    owner: &str,
    directory: &fastctx_types::layout::ConversationDir,
    kind: &'static str,
    batch: Vec<(PathBuf, Vec<u8>, archive::Signature)>,
) -> io::Result<usize> {
    if batch.is_empty() {
        return Ok(0);
    }
    let bytes = batch.iter().map(|(path, bytes, _)| path.as_os_str().len() + bytes.len()).sum();
    let owned = owner.to_owned();
    let batch = database.write_now_at(fastctx_storage::Priority::Background, bytes, move |connection| {
        for (path, bytes, _) in &batch {
            let name = name(path)?;
            connection.execute(
                "INSERT INTO composer_records(owner,kind,name,content,expires) VALUES(?1,?2,?3,?4,NULL) ON CONFLICT DO NOTHING",
                params![owned, kind, name, bytes]
            )?;
            let (stored, expires): (Vec<u8>, Option<i64>) = connection.query_row(
                "SELECT content,expires FROM composer_records WHERE owner=?1 AND kind=?2 AND name=?3",
                params![owned, kind, name], |row| Ok((row.get(0)?,row.get(1)?))
            )?;
            if expires.is_none() && stored != *bytes {
                return Err(Error::new("legacy composer receipt copies disagree; source preserved"));
            }
        }
        Ok(batch)
    }).map_err(io::Error::other)?;
    // The write answer means the FULL transaction committed. A concurrent live
    // operation can supersede the receipt, but does not remove its row here.
    // The original goes to the archive for its retention, not away.
    let mut count = 0;
    for (path, _, signature) in batch {
        let archived = crate::conversation_store::with_live_files(directory, || {
            archive::verified(world, owner, &path, &signature)
        });
        match archived {
            Ok(()) => count += 1,
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::WouldBlock) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}
