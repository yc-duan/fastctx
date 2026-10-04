//! Retires provable 0.x artifacts without stopping user work.
//!
//! The released installers did not record original contents for jobs, npm
//! caches, or customized settings. Those cannot be distinguished from later
//! user edits and are archived unchanged, with their locations in the report and the
//! durable `upgrade-clean.json` record. Host integration and `bin` belong to
//! `apply` and are never visited here.

mod archive;
mod files;
mod jobs;
mod leftovers;
mod paths;
mod processes;
mod record;
mod settings;
mod updates;

use std::path::{Path, PathBuf};

use crate::Notice;
use crate::apply::{Places, Report};
use crate::transaction::{FileChange, Snapshot, Transaction};

pub use crate::apply::Error;

#[derive(Default)]
struct Plan {
    edits: Vec<(Snapshot, Option<Vec<u8>>, String)>,
    report: Report,
    locks: Vec<std::fs::File>,
    archives: Vec<(archive::Object, &'static str, String)>,
    inventory: Option<Result<processes::Inventory, String>>,
    empty_directories: Vec<archive::Object>,
}

impl Plan {
    fn leave(&mut self, path: &Path, reason: impl Into<String>) {
        self.report.notices.push(Notice { location: path.display().to_string(), reason: reason.into() });
    }

    fn active(&mut self, path: &Path, reason: impl Into<String>) {
        let reason = reason.into();
        self.report.leftovers.push(format!("{}: {reason}", path.display()));
        self.leave(path, reason);
    }

    fn read(&mut self, path: &Path) -> Option<Snapshot> {
        match files::read(path) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                self.leave(path, error.to_string());
                None
            }
        }
    }

    fn delete(&mut self, snapshot: Snapshot, why: impl Into<String>) {
        self.edits.push((snapshot, None, why.into()));
    }

    fn has_work(&self) -> bool {
        !self.edits.is_empty()
            || !self.archives.is_empty()
            || !self.empty_directories.is_empty()
            || !self.report.notices.is_empty()
            || !self.report.leftovers.is_empty()
    }

    fn archive(&mut self, path: &Path, category: &'static str, reason: impl Into<String>) {
        match archive::Object::read(path) {
            Ok(Some(object)) => {
                if !self.archives.iter().any(|(existing, _, _)| existing.path == object.path) {
                    self.archives.push((object, category, reason.into()));
                }
            }
            Ok(None) => {}
            Err(error) => self.leave(path, error.to_string()),
        }
    }

    fn inventory(&mut self) -> Result<&processes::Inventory, &str> {
        self.inventory
            .get_or_insert_with(|| processes::Inventory::read().map_err(|error| error.to_string()))
            .as_ref()
            .map_err(String::as_str)
    }

    fn quiet(&mut self, path: &Path, old_root: &Path, fresh: bool) -> bool {
        if fresh {
            self.inventory = None;
        }
        let blockers = match self.inventory() {
            Ok(inventory) => inventory.legacy_blockers(old_root),
            Err(error) => vec![error.into()],
        };
        for reason in &blockers {
            self.active(path, reason.clone());
        }
        blockers.is_empty()
    }

    fn unused(&mut self, path: &Path, command_lines: bool) -> bool {
        let blockers = match self.inventory() {
            Ok(inventory) => inventory.users_of(path, command_lines),
            Err(error) => vec![error.into()],
        };
        for reason in &blockers {
            self.active(path, reason.clone());
        }
        blockers.is_empty()
    }

    fn creator_ended(&mut self, path: &Path, pid: u32) -> bool {
        match processes::dead(pid) {
            Ok(true) => true,
            Ok(false) => {
                self.active(path, format!("creator PID {pid} is still running; cleanup deferred"));
                false
            }
            Err(error) => {
                self.active(path, format!("cannot inspect creator PID {pid}: {error}"));
                false
            }
        }
    }

    fn empty_after(&mut self, path: &Path) {
        match archive::Object::read(path) {
            Ok(Some(object)) => {
                if !self.empty_directories.contains(&object) {
                    self.empty_directories.push(object);
                }
            }
            Ok(None) => {}
            Err(error) => self.leave(path, error.to_string()),
        }
    }
}

/// Deletes provably unchanged artifacts and archives uncertain contents under
/// `legacy-archive-YYYY-MM-DD`. Never signals a process or visits the stable `bin` copy.
///
/// Old versions used `HOME`, then `USERPROFILE`, ignoring `FASTCTX_HOME`.
/// Those legacy environment coordinates (including AppData/XDG/temp) are
/// deliberately used for discovery; `places.root` holds the new audit record.
/// Callers testing another home must supply that environment to the process.
/// Complete the legacy health handshake before calling this function.
///
/// No evidence means no writes. Repeated identical observations do not rewrite
/// the record. Each mutation is journaled before publication and rechecked on
/// recovery; cooperating new installers use the same lock as `apply`.
pub fn clean(places: &Places) -> Result<Report, Error> {
    let paths = paths::Paths::legacy()?;
    let record_path = places.root.join("upgrade-clean.json");
    let mut plan = discover(&paths, places);
    let previous = files::read(&record_path)?;
    if !plan.has_work() && !previous.exists() {
        return Ok(Report::default());
    }
    files::check_path(&places.root).map_err(io)?;
    if !places.root.exists() {
        fastctx_platform::ensure_private_dir(&places.root).map_err(io)?;
    }
    let lock_path = places.root.join("apply.lock");
    files::check_path(&lock_path).map_err(io)?;
    let _lock = fastctx_platform::FileLock::acquire(&lock_path).map_err(io)?;
    // Discovery precedes locking only to keep a fresh installation write-free.
    // Read again under the lock so apply's receipt retirement is not overwritten.
    drop(plan);
    plan = discover(&paths, places);
    let mut record = record::Record::open(&record_path)?;
    plan.report.notices.extend(record.recover(&paths, places)?);
    record.observe(&plan.report);
    record.save()?;
    for (before, after, why) in std::mem::take(&mut plan.edits) {
        let path = before.path().to_path_buf();
        let operation = record.prepare(&before, after.as_deref(), why)?;
        let change = match after {
            Some(bytes) => FileChange::write(before, bytes),
            None => FileChange::delete(before),
        };
        match Transaction::new([change]).commit() {
            Ok(receipt) => {
                plan.report.changed.extend(receipt.changed);
                for error in receipt.cleanup {
                    plan.report.leftovers.push(error.to_string());
                }
                record.complete(operation)?;
            }
            Err(error) => {
                let reason = error.to_string();
                plan.report.notices.push(Notice { location: path.display().to_string(), reason: reason.clone() });
                plan.report.leftovers.push(format!("{}: cleanup is deferred: {reason}", path.display()));
                record.deferred(operation, reason)?;
            }
        }
    }
    for (object, category, reason) in std::mem::take(&mut plan.archives) {
        let (index, destination, copying) = record.prepare_archive(&object, category, &reason, &places.root)?;
        let previous = record.archive_destinations();
        match archive::transfer(&object, &destination, &plan.locks, copying, &previous, || {
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
    plan.empty_directories.sort_by_key(|object| std::cmp::Reverse(object.path.components().count()));
    for object in &plan.empty_directories {
        if archive::Object::read(&object.path).map_err(io)?.as_ref() != Some(object) {
            continue;
        }
        if std::fs::read_dir(&object.path).map_err(io)?.next().transpose().map_err(io)?.is_some() {
            continue;
        }
        let operation = record.prepare_directory(object)?;
        match std::fs::remove_dir(&object.path) {
            Ok(()) => {
                plan.report.changed.push(object.path.clone());
                record.directory_finished(operation, "completed".into())?;
                crate::disk::sync_parent(&object.path).map_err(io)?;
            }
            Err(error) => {
                record.directory_finished(operation, format!("deferred: {error}"))?;
                plan.report.leftovers.push(format!("{}: {error}", object.path.display()));
            }
        }
    }
    record.observe(&plan.report);
    record.save()?;
    if record.wrote() {
        plan.report.changed.push(record_path);
    }
    Ok(plan.report)
}

fn discover(paths: &paths::Paths, places: &Places) -> Plan {
    let mut plan = Plan::default();
    jobs::inspect(&paths.root, &mut plan);
    settings::inspect(&paths.root, &mut plan);
    updates::inspect(&paths.root, &mut plan);
    paths.inspect_other(&mut plan);
    leftovers::inspect(places, &paths.root, &mut plan);
    plan
}

fn io(error: std::io::Error) -> Error {
    Error::Io(error.to_string())
}

fn entries(path: &Path, plan: &mut Plan) -> Vec<PathBuf> {
    match files::entries(path) {
        Ok(entries) => entries,
        Err(error) => {
            plan.leave(path, error.to_string());
            Vec::new()
        }
    }
}
