//! The SQL rejection intent records the quarantine destination before either
//! filesystem change. Restart therefore never invents a second missing offer.
use fastctx_platform::{disk, readings};
use fastctx_storage::{Database, Priority, rusqlite::OptionalExtension};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(super) fn keep(world: &Path, database: &Database, conversation: &str, reason: &str) -> io::Result<()> {
    let directory = super::directory(world, conversation).map_err(io::Error::other)?;
    let source = super::path(world, conversation).map_err(io::Error::other)?;
    super::super::with_file_gate(&directory, || {
        let current = database
            .read_background_now(|db| {
                Ok(db
                    .query_row(
                        "SELECT archive FROM node_conversation_transfers WHERE name=?1 AND rejected=0",
                        [conversation],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?)
            })
            .map_err(io::Error::other)?;
        let Some(kept) = current else {
            return Ok(());
        };
        let parent = crate::record_migration::archive::root(world).join("damaged").join(conversation);
        let mut target = kept.map(PathBuf::from);
        if let Some(path) = &target {
            if path.parent() != Some(parent.as_path()) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid import quarantine destination"));
            }
            if disk::exists(path)? {
                if !disk::exists(&source)? {
                    return Ok(());
                }
                target = None;
            }
        }
        let target = match target {
            Some(target) => target,
            None => {
                let mut nonce = [0u8; 16];
                getrandom::fill(&mut nonce).map_err(io::Error::other)?;
                let target = parent.join(format!("records.sqlite3.import-{:032x}", u128::from_be_bytes(nonce)));
                let (scope, kept) = (conversation.to_string(), target.to_string_lossy().into_owned());
                database
                    .write_now_at(Priority::Control, kept.len() + 128, move |db| {
                        db.execute(
                            "UPDATE node_conversation_transfers SET archive=?2 WHERE name=?1 AND rejected=0",
                            (scope, kept),
                        )?;
                        Ok(())
                    })
                    .map_err(io::Error::other)?;
                target
            }
        };
        fastctx_platform::ensure_private_dir(&parent)?;
        let note = target
            .with_file_name(format!("{}.why.json", target.file_name().expect("quarantine filename").to_string_lossy()));
        let missing = !disk::exists(&source)?;
        let fresh = !disk::exists(&note)?;
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "conversation":conversation,"family":"conversation-bundle","source":source,
            "reason":reason,"missing":missing,"at":jiff::Timestamp::now()
        }))?;
        fastctx_platform::publish(&note, &bytes, fastctx_platform::Permissions::Private)?;
        if !missing {
            disk::rename(&source, &target)?;
            readings::counter("migration.damaged_files").add(1);
        }
        if fresh {
            readings::gauge("migration.damaged_pending").add(1);
        }
        Ok(())
    })
}
