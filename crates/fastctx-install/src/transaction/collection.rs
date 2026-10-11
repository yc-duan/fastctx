//! Recovery objects have no reader after a conflict-free transaction settles.

use super::*;

/// Collects settled transactions under the caller's apply lock. The active
/// pointer and conflict evidence survive, including archives on other volumes.
pub fn collect_finished(root: &Path) -> Result<(), FileError> {
    let pointer = Snapshot::read(root.join("applied-files-pending.json"))?;
    let active = pointer
        .bytes()
        .map(serde_json::from_slice::<Active>)
        .transpose()
        .map_err(|error| invalid(pointer.path(), &error.to_string()))?;
    let history = match fs::canonicalize(root.join("apply-history")) {
        Ok(history) => history,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FileError::io(root, "locate transaction history", error)),
    };
    let active = active
        .map(|pointer| {
            if pointer.version != 1 {
                return Err(invalid(root, "unsupported active transaction"));
            }
            let manifest = fs::canonicalize(&pointer.manifest)
                .map_err(|error| FileError::io(&pointer.manifest, "locate active transaction", error))?;
            if manifest.parent().and_then(Path::parent) != Some(history.as_path()) {
                return Err(invalid(&manifest, "active transaction is outside this installation"));
            }
            Ok(manifest)
        })
        .transpose()?;
    let entries = match fs::read_dir(&history) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(FileError::io(&history, "list settled transactions", error)),
    };
    for entry in entries {
        let directory = entry.map_err(|error| FileError::io(&history, "list settled transactions", error))?.path();
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|error| FileError::io(&directory, "inspect transaction", error))?;
        if disk::is_link(&metadata) || !metadata.is_dir() {
            continue;
        }
        let manifest = directory.join("manifest.json");
        if !manifest.is_file() || active.as_ref().is_some_and(|active| *active == manifest) {
            continue;
        }
        let journal = Journal::read(&manifest)?;
        if journal.conflicts.is_empty()
            && matches!(journal.phase, Phase::Committed | Phase::RolledBack | Phase::Preparing)
        {
            discard(&journal)?;
        }
    }
    Ok(())
}

pub(super) fn discard(journal: &Journal) -> Result<(), FileError> {
    if !journal.conflicts.is_empty() {
        return Ok(());
    }
    let home = journal.manifest.parent().expect("manifest directory");
    let mut archives = vec![home.to_path_buf()];
    for entry in &journal.entries {
        let archive = entry.displaced.parent().expect("archive directory");
        if archive != home {
            let expected = format!(".fastctx-{}", home.file_name().expect("transaction name").to_string_lossy());
            if archive.file_name().is_none_or(|name| name != expected.as_str()) {
                return Err(invalid(archive, "unexpected cross-volume recovery archive"));
            }
            archives.push(archive.to_path_buf());
        }
    }
    archives.sort();
    archives.dedup();
    let mut failures = Vec::new();
    for archive in &archives {
        let entries = match fs::read_dir(archive) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(FileError::io(archive, "list recovery objects", error)),
        };
        let metadata =
            fs::symlink_metadata(archive).map_err(|error| FileError::io(archive, "inspect recovery archive", error))?;
        if disk::is_link(&metadata) {
            return Err(FileError::changed(archive));
        }
        for entry in entries {
            let path = entry.map_err(|error| FileError::io(archive, "list recovery objects", error))?.path();
            if path == journal.manifest {
                continue;
            }
            let metadata =
                fs::symlink_metadata(&path).map_err(|error| FileError::io(&path, "inspect recovery object", error))?;
            disk::ordinary(&metadata).map_err(|error| FileError::io(&path, "inspect recovery object", error))?;
            let removed = delete_object(journal, &path);
            if let Err(error) = removed {
                failures.push(error);
            }
        }
    }
    if let Some(first) = failures.first() {
        return Err(invalid(&first.path, &failures.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")));
    }
    for archive in archives.iter().filter(|archive| archive.as_path() != home) {
        match fs::remove_dir(archive) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(FileError::io(archive, "remove empty recovery archive", error));
            }
            _ => {}
        }
        disk::sync_parent(archive)
            .map_err(|error| FileError::io(archive, "sync recovery archive collection", error))?;
    }
    fs::remove_file(&journal.manifest)
        .map_err(|error| FileError::io(&journal.manifest, "discard settled manifest", error))?;
    fs::remove_dir(home).map_err(|error| FileError::io(home, "remove empty transaction", error))?;
    disk::sync_parent(home).map_err(|error| FileError::io(home, "sync transaction collection", error))
}

fn delete_object(journal: &Journal, path: &Path) -> Result<(), FileError> {
    let file = disk::open(path).map_err(|error| FileError::io(path, "open settled recovery object", error))?;
    match disk::delete(&file, path) {
        Ok(()) => Ok(()),
        Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
            let root =
                journal.manifest.parent().and_then(Path::parent).and_then(Path::parent).expect("transaction root");
            let bin = root.join("bin");
            let program = journal.entries.iter().any(|entry| {
                entry.target == bin.join(crate::renewal::EXECUTABLE)
                    && entry.target.parent() == Some(bin.as_path())
                    && ([entry.displaced.as_path(), entry.withdrawn.as_path()].contains(&path)
                        || [&entry.observed, &entry.after].into_iter().flatten().any(|object| object.path == path)
                        || entry
                            .restorations
                            .iter()
                            .any(|restoration| restoration.source.path == path || restoration.displaced == path)
                        || entry.displaced.parent().is_some_and(|archive| {
                            archive.join(entry.publication.file_name().expect("publication name")) == path
                        }))
            });
            if !program {
                return Err(FileError::io(path, "discard settled recovery object", error));
            }
            // NTFS needs one name for a mapped image. It is a live image
            // lease in bin, not recovery history or another program copy.
            crate::executable::retain_archived(&bin, path, &file)
                .map_err(|error| FileError::io(path, "retain mapped program name", error))?;
            disk::delete(&file, path).map_err(|error| FileError::io(path, "discard mapped recovery name", error))
        }
        Err(error) => Err(FileError::io(path, "discard settled recovery object", error)),
    }
}
