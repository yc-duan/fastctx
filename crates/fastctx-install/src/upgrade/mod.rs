//! Retires provable 0.x artifacts without stopping user work.
//!
//! The released installers did not record original contents for jobs, npm
//! caches, or customized settings. Those cannot be distinguished from later
//! user edits and are archived unchanged, with their locations in the report and the
//! durable `upgrade-clean.json` record. Host integration and `bin` belong to
//! `apply` and are never visited here.

mod archive;
mod descriptors;
mod files;
#[cfg(test)]
mod health;
mod jobs;
mod leftovers;
mod paths;
mod processes;
mod record;
mod run;
mod settings;
mod updates;

use std::path::{Path, PathBuf};

use crate::Notice;
use crate::apply::{Places, Report};
use crate::transaction::Snapshot;

pub use crate::apply::Error;

#[derive(Default)]
struct Plan {
    edits: Vec<(Snapshot, Option<Vec<u8>>, String)>,
    report: Report,
    locks: Vec<std::fs::File>,
    runtime_locks: std::collections::HashMap<PathBuf, Vec<std::fs::File>>,
    archives: Vec<(archive::Object, &'static str, String)>,
    archive_paths: std::collections::HashSet<PathBuf>,
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
                self.archive_object(object, category, reason.into());
            }
            Ok(None) => {}
            Err(error) => self.leave(path, error.to_string()),
        }
    }

    fn archive_object(&mut self, object: archive::Object, category: &'static str, reason: String) {
        if self.archive_paths.insert(object.path.clone()) {
            self.archives.push((object, category, reason));
        }
    }

    fn inventory(&mut self) -> Result<&processes::Inventory, &str> {
        self.inventory
            .get_or_insert_with(|| processes::Inventory::read().map_err(|error| error.to_string()))
            .as_ref()
            .map_err(String::as_str)
    }

    fn quiet(&mut self, path: &Path, fresh: bool) -> bool {
        if fresh {
            self.inventory = None;
        }
        let blockers = match self.inventory() {
            Ok(inventory) => inventory.legacy_blockers(),
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

/// What the cleanup has done so far, as its record says: for telling the
/// user where the old things went (`world-upgrade.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// The record of every step, `upgrade-clean.json`.
    pub record: PathBuf,
    /// The `legacy-archive-<day>` directories things were put in.
    pub archives: Vec<PathBuf>,
    pub archived: usize,
    /// Files removed or rewritten because they were exactly as an old
    /// version wrote them.
    pub tidied: usize,
    /// Left in place until nothing uses them any more.
    pub waiting: usize,
}

/// The cleanup record under `places.root`; none when nothing old was found.
pub fn summary(places: &Places) -> Result<Option<Summary>, Error> {
    record::summary(&places.root.join("upgrade-clean.json"), &places.root)
}

/// Whether a World rooted at `root` is where the old versions lived: they
/// always wrote under the user's own `.fastctx`, whatever `FASTCTX_HOME`
/// said, so a World kept elsewhere has no old installation of its own to
/// clean up.
pub fn is_legacy_root(root: &Path) -> std::io::Result<bool> {
    let legacy = fastctx_platform::user_home()?.join(".fastctx");
    match (std::fs::canonicalize(root), std::fs::canonicalize(&legacy)) {
        (Ok(root), Ok(legacy)) => Ok(root == legacy),
        (Err(error), _) | (_, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        (Err(error), _) | (_, Err(error)) => Err(error),
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
/// recovery. Cleanup has its own lock; only shared settings use the `apply` lock.
pub fn clean(places: &Places) -> Result<Report, Error> {
    let paths = paths::Paths::legacy()?;
    descriptors::with_capacity(paths.runtime_entry_count().map_err(io)?, || clean_with(&paths, places))
}

fn clean_with(paths: &paths::Paths, places: &Places) -> Result<Report, Error> {
    let record_path = places.root.join("upgrade-clean.json");
    if !record_path.try_exists().map_err(io)? {
        let mut evidence = discover(paths, places);
        settings::inspect(&paths.root, &mut evidence);
        if !evidence.has_work() {
            return Ok(Report::default());
        }
    }
    files::check_path(&places.root).map_err(io)?;
    if !places.root.try_exists().map_err(io)? {
        crate::disk::ensure_private_dir(&places.root).map_err(io)?;
    }
    let lock_path = places.root.join("upgrade-clean.lock");
    files::check_path(&lock_path).map_err(io)?;
    let _cleanup = fastctx_platform::FileLock::acquire(&lock_path).map_err(io)?;
    let mut record = record::Record::open(&record_path)?;
    let mut report = Report::default();
    report.notices.extend(record.recover(paths, places)?);

    // Only legacy settings share a write target with apply. Discover them
    // under its lock, then release it before runtime inspection or archival.
    {
        let lock_path = places.root.join("apply.lock");
        files::check_path(&lock_path).map_err(io)?;
        let _apply = fastctx_platform::FileLock::acquire(&lock_path).map_err(io)?;
        let mut settings = Plan::default();
        settings::inspect(&paths.root, &mut settings);
        run::execute(&mut settings, &mut record, places)?;
        report.changed.extend(settings.report.changed);
        report.notices.extend(settings.report.notices);
        report.leftovers.extend(settings.report.leftovers);
    }
    let mut plan = discover(paths, places);
    run::execute(&mut plan, &mut record, places)?;
    report.changed.extend(plan.report.changed);
    report.notices.extend(plan.report.notices);
    report.leftovers.extend(plan.report.leftovers);
    record.observe(&report);
    record.save()?;
    if record.wrote() {
        report.changed.push(record_path);
    }
    Ok(report)
}

fn discover(paths: &paths::Paths, places: &Places) -> Plan {
    let mut plan = Plan::default();
    jobs::inspect(&paths.root, &mut plan);
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
