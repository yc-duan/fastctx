//! Originals the migration set aside although every record in them was
//! already stored with the same value. Earlier versions kept a job's write
//! facts twice, as lines of its journal and as files of their own, and the
//! migration told the two apart by the journal's line end alone. Such an
//! original goes with the verified ones, kept as long, and its explanation
//! goes, so the machine no longer tells of records lost that are not.
use fastctx_platform::disk;
use fastctx_storage::{Database, rusqlite::OptionalExtension};
use serde::Deserialize;
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Deserialize)]
struct Why {
    conversation: String,
    family: String,
    reason: String,
}

/// Settles what it can, and gives the number settled with the last failure
/// met: one original it could not settle leaves the rest to go on.
pub(crate) fn settle(database: &Database, world: &Path) -> (u64, Option<io::Error>) {
    let versions = match disk::read_dir(world.join("archive")) {
        Ok(versions) => versions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (0, None),
        Err(error) => return (0, Some(error)),
    };
    let (mut settled, mut failure) = (0, None);
    for version in versions {
        let version = match version.and_then(|entry| Ok((entry.path(), entry.file_type()?.is_dir()))) {
            Ok((path, true)) => path,
            Ok(_) => continue,
            Err(error) => {
                failure = Some(error);
                continue;
            }
        };
        let mut notes = Vec::new();
        if let Err(error) = notes_under(&version.join("damaged"), &mut notes) {
            failure = Some(error);
        }
        for note in notes {
            match one(database, world, &version, &note) {
                Ok(true) => settled += 1,
                Ok(false) => {}
                Err(error) => failure = Some(error),
            }
        }
    }
    (settled, failure)
}

fn notes_under(path: &Path, notes: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match disk::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            notes_under(&entry.path(), notes)?;
        } else if entry.file_name().to_string_lossy().ends_with(".why.json") {
            notes.push(entry.path());
        }
    }
    Ok(())
}

fn one(database: &Database, world: &Path, version: &Path, note: &Path) -> io::Result<bool> {
    let Ok(why) = serde_json::from_slice::<Why>(&disk::read(note)?) else { return Ok(false) };
    let name = note.file_name().expect("explanation filename").to_string_lossy();
    let original = note.with_file_name(name.strip_suffix(".why.json").expect("an explanation"));
    let relative = original
        .strip_prefix(version.join("damaged"))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a damaged original outside its archive"))?;
    let target = version.join(relative);
    let bytes = match disk::read(&original) {
        Ok(bytes) => bytes,
        // Settled before, and stopped before its explanation went: it goes
        // now. An original not yet set aside, its explanation written first,
        // is in neither place.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if !disk::exists(&target)? {
                return Ok(false);
            }
            disk::remove_file(note)?;
            return Ok(true);
        }
        Err(error) => return Err(error),
    };
    let Some(records) = records(&why, &bytes) else { return Ok(false) };
    let (conversation, family) = (why.conversation.clone(), why.family.clone());
    let identities: Vec<String> = records.iter().map(|(identity, _)| identity.clone()).collect();
    let stored = database
        .read_background_now(move |db| {
            let mut read = db.prepare_cached(
                "SELECT body FROM conversation_records WHERE conversation=?1 AND family=?2 AND identity=?3",
            )?;
            let mut bodies = Vec::with_capacity(identities.len());
            for identity in &identities {
                bodies.push(
                    read.query_row((&conversation, &family, identity), |row| row.get::<_, Vec<u8>>(0)).optional()?,
                );
            }
            Ok(bodies)
        })
        .map_err(io::Error::other)?;
    let same = records.iter().zip(&stored).all(|((_, value), body)| {
        body.as_deref().and_then(|body| serde_json::from_slice::<Value>(body).ok()).is_some_and(|body| body == *value)
    });
    if !same {
        return Ok(false);
    }
    let file = || {
        fastctx_platform::ensure_private_dir(target.parent().expect("archived parent"))?;
        // Kept as long as a verified original, from now.
        disk::open_with(std::fs::OpenOptions::new().write(true), &original)?.set_modified(SystemTime::now())?;
        disk::rename(&original, &target)?;
        disk::remove_file(note)
    };
    if why.conversation.starts_with('@') {
        file()?;
    } else {
        let directory =
            fastctx_types::layout::ConversationDir::at(&world.join("conversations").join(&why.conversation));
        crate::conversation_store::with_live_files(&directory, file)?;
    }
    Ok(true)
}

/// The records an original holds, each with its identity: a write fact by
/// its id, alone or a journal of them; otherwise the one row a conflict
/// names, when the original is that row and nothing else.
fn records(why: &Why, bytes: &[u8]) -> Option<Vec<(String, Value)>> {
    if why.family.starts_with("jobs/") && why.family.ends_with("/writes") {
        let values = match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => vec![value],
            Err(_) => bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
                .map(serde_json::from_slice::<Value>)
                .collect::<Result<Vec<_>, _>>()
                .ok()?,
        };
        let records = values
            .into_iter()
            .map(|value| Some((value.get("id")?.as_str()?.to_string(), value)))
            .collect::<Option<Vec<_>>>()?;
        return (!records.is_empty()).then_some(records);
    }
    let identity = why.reason.strip_prefix("legacy row ")?.strip_suffix(" conflicts with its stored value")?;
    Some(vec![(identity.to_string(), serde_json::from_slice(bytes).ok()?)])
}
