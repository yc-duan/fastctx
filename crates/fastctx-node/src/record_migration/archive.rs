//! Verified legacy bytes are archived, not discarded, for `RETENTION`.
//! Damaged inputs are kept as long beside an explanation, and counted while
//! they are, which the machine's health tells.
use fastctx_platform::{disk, readings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

mod damaged;
#[cfg(feature = "storage-drill")]
pub(crate) use damaged::drill as damaged_drill;
mod duplicates;
pub(super) use duplicates::settle as settle_duplicates;

pub(crate) const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
static DAMAGED: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("migration.damaged_files"));
pub(crate) static DAMAGED_KEPT: LazyLock<&'static readings::Gauge> =
    LazyLock::new(|| readings::gauge("migration.damaged_kept"));
static ARCHIVED: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("migration.archived_files"));
static REMOVED: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("migration.expired_files"));

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Signature {
    pub bytes: u64,
    pub digest: String,
}

pub(crate) fn fingerprint(path: &Path) -> io::Result<Signature> {
    let mut file = disk::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        hash.update(&buffer[..read]);
    }
    Ok(Signature { bytes, digest: format!("{:x}", hash.finalize()) })
}

pub(crate) fn root(world: &Path) -> PathBuf {
    world.join("archive").join(env!("CARGO_PKG_VERSION"))
}

pub(super) fn relative(world: &Path, conversation: &str, source: &Path) -> io::Result<PathBuf> {
    if Path::new(conversation).components().count() != 1
        || source.components().any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid legacy source path"));
    }
    let owner = world.join("conversations").join(conversation);
    source
        .strip_prefix(&owner)
        .or_else(|_| source.strip_prefix(world))
        .map(Path::to_path_buf)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "legacy source is outside its world"))
}

pub(crate) fn verified_path(
    world: &Path,
    conversation: &str,
    source: &Path,
    expected: &Signature,
) -> io::Result<PathBuf> {
    let relative = relative(world, conversation, source)?;
    let original = root(world).join(conversation).join(relative);
    Ok(original.with_file_name(format!(
        "{}.{}",
        original.file_name().expect("legacy filename").to_string_lossy(),
        expected.digest
    )))
}

pub(crate) fn verified(world: &Path, conversation: &str, source: &Path, expected: &Signature) -> io::Result<()> {
    let target = verified_path(world, conversation, source, expected)?;
    match fingerprint(&target) {
        Ok(stored) if stored != *expected => {
            return Err(io::Error::other("the archived source no longer matches its verified bytes"));
        }
        Ok(_) => {
            match fingerprint(source) {
                Ok(current) if current == *expected => disk::remove_file(source)?,
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "the legacy source changed after verification",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if fingerprint(source)? != *expected {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "the legacy source changed after verification"));
    }
    fastctx_platform::ensure_private_dir(target.parent().expect("archive parent"))?;
    // Retention starts at archival, independently of how old the original was.
    disk::open_with(std::fs::OpenOptions::new().write(true), source)?.set_modified(SystemTime::now())?;
    disk::rename(source, &target)?;
    ARCHIVED.add(1);
    Ok(())
}

pub(crate) fn damaged(world: &Path, conversation: &str, family: &str, source: &Path, reason: &str) -> io::Result<()> {
    if matches!(conversation, "@node" | "@book") {
        return damaged_file(world, conversation, family, source, reason);
    }
    let directory = fastctx_types::layout::ConversationDir::at(&world.join("conversations").join(conversation));
    crate::conversation_store::with_live_files(&directory, || damaged_file(world, conversation, family, source, reason))
}

/// Layout repair cannot call the ordinary gate, which first waits for that same
/// layout to become usable. Its registered database still enforces deletion.
pub(crate) fn layout_metadata(
    world: &Path,
    database: &fastctx_storage::Database,
    conversation: &str,
    source: &Path,
    reason: &str,
) -> io::Result<()> {
    let directory = fastctx_types::layout::ConversationDir::at(&world.join("conversations").join(conversation));
    crate::conversation_store::with_file_gate(&directory, || {
        database
            .read_background_now(|db| crate::conversation_store::check(db, conversation))
            .map_err(io::Error::other)?;
        damaged_file(world, conversation, "layout-metadata", source, reason)
    })
}

/// Sets aside a file found where a finished import's bundle was. The import
/// holding the conversation runs this itself, so it takes only the file gate.
pub(crate) fn leftover(world: &Path, conversation: &str, source: &Path, reason: &str) -> io::Result<()> {
    let directory = fastctx_types::layout::ConversationDir::at(&world.join("conversations").join(conversation));
    crate::conversation_store::with_file_gate(&directory, || {
        damaged_file(world, conversation, "conversation-bundle", source, reason)
    })
}

fn damaged_file(world: &Path, conversation: &str, family: &str, source: &Path, reason: &str) -> io::Result<()> {
    let relative = relative(world, conversation, source)?;
    let original = root(world).join("damaged").join(conversation).join(relative);
    damaged::quarantine(&original, conversation, family, source, reason)
}

pub(crate) fn collect(world: &Path, retention: Duration) -> io::Result<u64> {
    fn walk(path: &Path, retention: Duration, count: &mut u64) -> io::Result<()> {
        let entries = match disk::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_name() == "damaged" {
                continue;
            }
            let metadata = disk::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                walk(&entry.path(), retention, count)?;
                match disk::remove_dir(entry.path()) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(error.kind(), io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound) => {}
                    Err(error) => return Err(error),
                }
            } else if metadata.is_file() && metadata.modified()?.elapsed().is_ok_and(|elapsed| elapsed >= retention) {
                disk::remove_file(entry.path())?;
                *count += 1;
                REMOVED.add(1);
                if (*count).is_multiple_of(128) {
                    std::thread::yield_now();
                }
            }
        }
        Ok(())
    }
    let mut count = 0;
    walk(&world.join("archive"), retention, &mut count)?;
    Ok(count)
}

pub(crate) fn delete_conversation(world: &Path, conversation: &str) -> io::Result<()> {
    let versions = match disk::read_dir(world.join("archive")) {
        Ok(versions) => versions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for version in versions {
        let version = version?.path();
        if !disk::symlink_metadata(&version)?.is_dir() {
            continue;
        }
        for path in [version.join(conversation), version.join("damaged").join(conversation)] {
            fastctx_platform::owned_directory::delete(&path)?;
        }
    }
    Ok(())
}

/// The damaged originals kept, each with its explanation; counted again
/// after a process restart.
#[cfg(feature = "storage-drill")]
pub(crate) fn damaged_count(world: &Path) -> io::Result<u64> {
    damaged_kept(world, Duration::MAX)
}

/// Removes the damaged originals set aside `retention` ago or longer, each
/// with its explanation, and counts those kept. Only the `damaged`
/// directories are walked, however many verified originals the archive
/// holds.
pub(crate) fn damaged_kept(world: &Path, retention: Duration) -> io::Result<u64> {
    fn walk(path: &Path, retention: Duration, kept: &mut u64) -> io::Result<()> {
        let entries = match disk::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(&path, retention, kept)?;
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(original) = name.strip_suffix(".why.json") else {
                damaged::recover(&path)?;
                continue;
            };
            // The explanation is written when its original is set aside.
            if disk::symlink_metadata(&path)?.modified()?.elapsed().is_ok_and(|age| age >= retention) {
                // The original first: an explanation left alone by a stop in
                // between goes the next time.
                match disk::remove_file(path.with_file_name(original)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
                disk::remove_file(&path)?;
                REMOVED.add(1);
            } else {
                *kept += 1;
            }
        }
        Ok(())
    }
    let versions = match disk::read_dir(world.join("archive")) {
        Ok(versions) => versions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            DAMAGED_KEPT.set(0);
            return Ok(0);
        }
        Err(error) => return Err(error),
    };
    let mut kept = 0;
    for version in versions {
        let version = version?.path();
        if disk::symlink_metadata(&version)?.is_dir() {
            walk(&version.join("damaged"), retention, &mut kept)?;
        }
    }
    DAMAGED_KEPT.set(kept as i64);
    Ok(kept)
}
