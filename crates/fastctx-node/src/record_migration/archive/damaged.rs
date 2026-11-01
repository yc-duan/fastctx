//! A durable quarantine destination joins an explanation to its original across
//! process death. A later file at the same source path gets a new destination.
use super::{DAMAGED, DAMAGED_KEPT, disk};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

const PENDING: &str = ".quarantine-pending";
static SOURCES: std::sync::LazyLock<crate::record_gates::Gates> = std::sync::LazyLock::new(|| {
    crate::record_gates::Gates::new(fastctx_platform::doing::Rank::ARCHIVING, "quarantine source")
});

#[derive(Serialize, Deserialize)]
struct Pending {
    source: std::path::PathBuf,
    target: String,
    explanation: serde_json::Value,
}

fn note(target: &Path) -> std::path::PathBuf {
    target.with_file_name(format!("{}.why.json", target.file_name().expect("damaged filename").to_string_lossy()))
}

fn target(journal: &Path, pending: &Pending) -> io::Result<std::path::PathBuf> {
    let mut components = Path::new(&pending.target).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid quarantine destination"));
    }
    Ok(journal.parent().expect("quarantine parent").join(&pending.target))
}

fn publish_note(target: &Path, pending: &Pending) -> io::Result<()> {
    fastctx_platform::publish(
        &note(target),
        &serde_json::to_vec_pretty(&pending.explanation)?,
        fastctx_platform::Permissions::Private,
    )
}

fn prepare(
    original: &Path,
    conversation: &str,
    family: &str,
    source: &Path,
    reason: &str,
) -> io::Result<std::path::PathBuf> {
    fastctx_platform::ensure_private_dir(original.parent().expect("damaged parent"))?;
    let name = original.file_name().expect("damaged filename").to_string_lossy();
    let journal = original.with_file_name(format!("{name}{PENDING}"));
    if !disk::exists(&journal)? {
        let at = jiff::Timestamp::now();
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let pending = Pending {
            source: source.to_path_buf(),
            target: format!("{name}.{}.{:032x}", at.as_nanosecond(), u128::from_be_bytes(nonce)),
            explanation: serde_json::json!({
                "conversation": conversation, "family": family,
                "source": source, "reason": reason, "at": at,
            }),
        };
        fastctx_platform::publish(&journal, &serde_json::to_vec(&pending)?, fastctx_platform::Permissions::Private)?;
    }
    Ok(journal)
}

pub(super) fn quarantine(
    original: &Path,
    conversation: &str,
    family: &str,
    source: &Path,
    reason: &str,
) -> io::Result<()> {
    let gate = SOURCES.get(source);
    let _source = gate.lock();
    loop {
        let journal = prepare(original, conversation, family, source, reason)?;
        let pending: Pending = serde_json::from_slice(&disk::read(&journal)?)?;
        let target = target(&journal, &pending)?;
        publish_note(&target, &pending)?;
        if disk::exists(&target)? {
            disk::remove_file(&journal)?;
            if disk::exists(source)? {
                continue;
            }
            return Ok(());
        }
        match disk::rename(source, &target) {
            Ok(()) => {
                DAMAGED.add(1);
                DAMAGED_KEPT.add(1);
                disk::remove_file(&journal)?;
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // No original was moved. Do not report an empty quarantine.
                disk::remove_file(note(&target))?;
                disk::remove_file(&journal)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}

/// Completes the bookkeeping when the original moved just before process death.
pub(super) fn recover(path: &Path) -> io::Result<()> {
    if !path.file_name().is_some_and(|name| name.to_string_lossy().ends_with(PENDING)) {
        return Ok(());
    }
    let Some(pending) = read_pending(path)? else { return Ok(()) };
    let gate = SOURCES.get(&pending.source);
    let _source = gate.lock();
    let Some(pending) = read_pending(path)? else { return Ok(()) };
    if disk::exists(target(path, &pending)?)? {
        // The explanation preceded the rename. Cleanup never recreates files
        // that a concurrent conversation deletion may already have removed.
        match disk::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

fn read_pending(path: &Path) -> io::Result<Option<Pending>> {
    match disk::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(feature = "storage-drill")]
pub(crate) fn drill(root: &Path) -> io::Result<()> {
    let source = root.join("world/timers/broken.json");
    let world = root.join("world");
    disk::create_dir_all(source.parent().expect("source parent"))?;
    let original = super::root(&world).join("damaged/@node/timers/broken.json");
    disk::write(&source, b"first")?;
    let journal = prepare(&original, "@node", "timers", &source, "broken timer")?;
    let pending: Pending = serde_json::from_slice(&disk::read(&journal)?)?;
    let first = target(&journal, &pending)?;
    publish_note(&first, &pending)?;
    quarantine(&original, "@node", "timers", &source, "broken timer")?;
    assert_eq!(disk::read(&first)?, b"first");
    assert_eq!(super::damaged_count(&world)?, 1);
    assert!(!disk::exists(&journal)?);
    disk::write(&source, b"second")?;
    let journal = prepare(&original, "@node", "timers", &source, "another broken timer")?;
    let pending: Pending = serde_json::from_slice(&disk::read(&journal)?)?;
    let second = target(&journal, &pending)?;
    publish_note(&second, &pending)?;
    disk::rename(&source, &second)?;
    assert_eq!(super::damaged_count(&world)?, 2);
    assert!(!disk::exists(&journal)?);
    disk::write(&source, b"third")?;
    quarantine(&original, "@node", "timers", &source, "yet another broken timer")?;
    assert_eq!(super::damaged_count(&world)?, 3);
    assert_eq!(disk::read(first)?, b"first");
    assert_eq!(disk::read(second)?, b"second");
    println!(
        "quarantine: explanation-before-rename and rename-before-cleanup resumed once; later source kept separately"
    );
    Ok(())
}
