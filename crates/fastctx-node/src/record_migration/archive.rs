//! Verified legacy bytes are archived, not discarded. Damaged inputs keep an
//! adjacent explanation and are never removed by ordinary archive retention.
use fastctx_platform::{disk, readings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

pub(crate) const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
static DAMAGED: LazyLock<&'static readings::Counter> = LazyLock::new(|| readings::counter("migration.damaged_files"));
static DAMAGED_PENDING: LazyLock<&'static readings::Gauge> =
    LazyLock::new(|| readings::gauge("migration.damaged_pending"));
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

fn relative(world: &Path, conversation: &str, source: &Path) -> io::Result<PathBuf> {
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

fn damaged_file(world: &Path, conversation: &str, family: &str, source: &Path, reason: &str) -> io::Result<()> {
    let relative = relative(world, conversation, source)?;
    let original = root(world).join("damaged").join(conversation).join(relative);
    let at = jiff::Timestamp::now();
    let target = original.with_file_name(format!(
        "{}.{}",
        original.file_name().expect("damaged filename").to_string_lossy(),
        at.as_nanosecond()
    ));
    let note =
        target.with_file_name(format!("{}.why.json", target.file_name().expect("damaged filename").to_string_lossy()));
    fastctx_platform::ensure_private_dir(target.parent().expect("damaged parent"))?;
    let explanation = serde_json::to_vec_pretty(&serde_json::json!({
        "conversation": conversation, "family": family,
        "source": source, "reason": reason, "at": at,
    }))?;
    fastctx_platform::publish(&note, &explanation, fastctx_platform::Permissions::Private)?;
    match disk::rename(source, &target) {
        Ok(()) => {
            DAMAGED.add(1);
            DAMAGED_PENDING.add(1);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            DAMAGED.add(1);
            DAMAGED_PENDING.add(1);
            Ok(())
        }
        Err(error) => Err(error),
    }
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

/// Retained explanations remain visible after a process restart.
pub(crate) fn damaged_count(world: &Path) -> io::Result<u64> {
    fn count(path: &Path) -> io::Result<u64> {
        let entries = match disk::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut result = 0;
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                result += count(&entry.path())?;
            } else if entry.file_name().to_string_lossy().ends_with(".why.json") {
                result += 1;
            }
        }
        Ok(result)
    }
    let result = count(&world.join("archive"))?;
    DAMAGED_PENDING.set(result as i64);
    Ok(result)
}
