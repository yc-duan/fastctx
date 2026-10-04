//! Publishes the whole write-ahead plan before executing any of its steps.

use crate::apply::Places;
use crate::transaction::{FileChange, Transaction};

use super::{Error, Plan, archive, files, io, record::Record};

pub(super) fn execute(plan: &mut Plan, record: &mut Record, places: &Places) -> Result<(), Error> {
    record.begin_batch();
    let mut edits = Vec::new();
    for (before, after, why) in std::mem::take(&mut plan.edits) {
        let operation = record.prepare(&before, after.as_deref(), why)?;
        edits.push((operation, before, after));
    }
    let mut archives = Vec::new();
    for (object, category, reason) in std::mem::take(&mut plan.archives) {
        for (child, category, reason) in record.inside(&object) {
            if archive::Object::read(&child.path).map_err(io)?.as_ref() != Some(&child) {
                continue;
            }
            let (index, destination, copying) = record.prepare_archive(&child, &category, &reason, &places.root)?;
            archives.push((index, child, destination, copying, reason));
        }
        let (index, destination, copying) = record.prepare_archive(&object, category, &reason, &places.root)?;
        archives.push((index, object, destination, copying, reason));
    }
    record.observe(&plan.report);
    record.save()?;

    for (operation, before, after) in edits {
        let path = before.path().to_path_buf();
        let change = match after {
            Some(bytes) => FileChange::write(before, bytes),
            None => FileChange::delete(before),
        };
        match Transaction::new([change]).commit() {
            Ok(receipt) => {
                plan.report.changed.extend(receipt.changed);
                plan.report.leftovers.extend(receipt.cleanup.into_iter().map(|error| error.to_string()));
                record.complete(operation)?;
            }
            Err(error) => {
                plan.active(&path, format!("cleanup deferred: {error}"));
                record.deferred(operation, error.to_string())?;
            }
        }
    }
    let previous = record.archive_destinations();
    for (index, object, destination, copying, reason) in archives {
        let release_runtime = cfg!(windows)
            && !copying
            && plan.runtime_locks.contains_key(&object.path)
            && !destination.parent().expect("archive slot").join("copy-manifest.json").try_exists().map_err(io)?
            && crate::disk::identity(&crate::disk::open_directory(&places.root).map_err(io)?).map_err(io)?.volume
                == object.identity.volume;
        if release_runtime {
            // Windows refuses a directory rename while any descendant handle
            // is open, including our own locks (Windows 11, 2026-10-04).
            // Release ours only for a same-volume move: a competing holder
            // makes the rename fail atomically. Cross-volume copies keep locks.
            plan.runtime_locks.remove(&object.path);
        }
        let locks: Vec<_> = plan.locks.iter().chain(plan.runtime_locks.values().flatten()).collect();
        match archive::transfer(&object, &destination, &locks, copying, &previous, || {
            if release_runtime {
                return Err(std::io::Error::other("runtime archive volume changed; source retained for next startup"));
            }
            record.archive_copying(index).map_err(std::io::Error::other)
        }) {
            Ok(()) => {
                record.archive_complete(index)?;
                plan.report.changed.extend([object.path.clone(), destination.clone()]);
                plan.leave(&object.path, format!("archived unchanged at {}; {reason}", destination.display()));
                for path in files::entries(destination.parent().expect("archive slot")).map_err(io)? {
                    if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".incomplete-copy-")) {
                        plan.leave(&path, "interrupted copy retained alongside the complete archive; original source mapped in upgrade-clean.json");
                    }
                }
            }
            Err(error) => {
                record.archive_deferred(index, &error)?;
                plan.active(&object.path, format!("archive deferred; destination {}: {error}", destination.display()));
            }
        }
    }

    let mut directories = Vec::new();
    let mut retiring = std::collections::HashSet::new();
    plan.empty_directories.sort_by_key(|object| std::cmp::Reverse(object.path.components().count()));
    for object in &plan.empty_directories {
        if archive::Object::read(&object.path).map_err(io)?.as_ref() != Some(object) {
            continue;
        }
        if files::entries(&object.path).map_err(io)?.iter().any(|path| !retiring.contains(path)) {
            continue;
        }
        let operation = record.prepare_directory(object)?;
        directories.push((operation, object.clone()));
        retiring.insert(object.path.clone());
    }
    // Completion and empty-directory intentions share this checkpoint. If it
    // cannot be published, the earlier pending plan still describes every move.
    record.observe(&plan.report);
    record.save()?;
    for (operation, object) in directories {
        if archive::Object::read(&object.path).map_err(io)?.as_ref() != Some(&object) {
            record.directory_finished(operation, "directory changed or absent; left untouched".into())?;
            continue;
        }
        if std::fs::read_dir(&object.path).map_err(io)?.next().transpose().map_err(io)?.is_some() {
            record.directory_finished(operation, "directory is not empty; retained".into())?;
            continue;
        }
        match std::fs::remove_dir(&object.path) {
            Ok(()) => {
                plan.report.changed.push(object.path.clone());
                crate::disk::sync_parent(&object.path).map_err(io)?;
                record.directory_finished(operation, "completed".into())?;
            }
            Err(error) => {
                record.directory_finished(operation, format!("deferred: {error}"))?;
                plan.active(&object.path, error.to_string());
            }
        }
    }
    record.observe(&plan.report);
    record.save()
}
