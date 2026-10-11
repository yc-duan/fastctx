//! Optimistic, compensating transactions for host configuration and install records.
//!
//! Every publication is atomic; the group is not atomically visible. On a returned
//! failure, completed changes are undone in reverse order, provided their installed
//! revisions are still present. The non-durable methods retain originals only in
//! memory and cannot recover a terminated process. The durable methods write a
//! journal before publication, retain the actual displaced objects, and let
//! `recover` complete publication or rollback after interruption.
//!
//! Attribute preservation covers Unix ownership, mode and extended attributes;
//! Windows owner/group, DACL, creation time, basic file flags and named streams.
//! Windows flags needing separate filesystem operations (including encryption,
//! compression and sparse allocation) fail explicitly before publication.
//! Rollback also restores the original modification time. File identities change
//! on replacement; access times are neither revisions nor restoration targets.
//!
//! Callers must serialize cooperating installers (for example with
//! `fastctx_platform::lock::FileLock`). A final comparison immediately precedes
//! each rename or unlink, but portable filesystems have no content-based conditional
//! rename: an uncooperative writer can still race that final system call.
//! Durable publication detects changed displaced objects after the system call,
//! restores when possible, and permanently preserves all displaced versions.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use crate::disk::Identity;
use crate::disk::{self, Staged};
use crate::executable;
pub use crate::snapshot::Snapshot;
use crate::snapshot::state_of_staged;

mod durable;
pub use durable::pending_record;
pub(crate) use durable::prepare_settings_command;
pub use durable::recover;
pub use durable::{collect_finished, prune_owned, retained_archives};

#[derive(Debug, thiserror::Error)]
pub enum Problem {
    #[error("changed since it was read; left untouched")]
    Changed,
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Invalid(&'static str),
}

#[derive(Debug, thiserror::Error)]
#[error("{}: {}: {}", path.display(), operation, problem)]
pub struct FileError {
    pub path: PathBuf,
    pub operation: &'static str,
    #[source]
    pub problem: Problem,
}

impl FileError {
    pub(crate) fn io(path: &Path, operation: &'static str, error: io::Error) -> Self {
        Self { path: path.into(), operation, problem: Problem::Io(error) }
    }

    pub(crate) fn changed(path: &Path) -> Self {
        Self { path: path.into(), operation: "compare", problem: Problem::Changed }
    }
}

/// Contents are intentionally not included in Debug output.
pub struct FileChange {
    before: Snapshot,
    after: Option<Arc<[u8]>>,
    mode: u32,
    executable: bool,
}

impl FileChange {
    /// Preserves existing attributes. A newly created user file gets the
    /// platform's ordinary permissions (0666 subject to umask on Unix).
    pub fn write(before: Snapshot, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self { before, after: Some(bytes.into()), mode: 0o666, executable: false }
    }

    /// A new private record gets mode 0600 on Unix. On Windows it inherits
    /// its directory's ACL; the caller must place it in a private directory.
    /// Existing files retain their existing permissions on both platforms.
    pub fn private(before: Snapshot, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self { mode: 0o600, ..Self::write(before, bytes) }
    }

    pub fn delete(before: Snapshot) -> Self {
        Self { before, after: None, mode: 0o666, executable: false }
    }

    /// Freezes a complete executable copy before any transaction writes.
    /// Existing destination attributes are preserved; a new Unix copy is 0755
    /// subject to umask. This opts only this file into Windows image retention.
    pub fn copy_executable(before: Snapshot, source: impl AsRef<Path>) -> Result<Self, FileError> {
        let source = Snapshot::read(source)?;
        let state = source
            .state
            .ok_or_else(|| FileError::io(&source.path, "read executable", io::ErrorKind::NotFound.into()))?;
        Ok(Self { before, after: Some(state.bytes), mode: 0o755, executable: true })
    }

    /// Deletes an installed executable, retaining a Windows image link while
    /// an existing process still maps it.
    pub fn delete_executable(before: Snapshot) -> Self {
        Self { executable: true, ..Self::delete(before) }
    }

    fn changes(&self) -> bool {
        self.before.bytes() != self.after.as_deref()
    }

    fn preflight(&self) -> Result<(), FileError> {
        self.before.verify()?;
        if self.changes() && self.before.state.as_ref().is_some_and(|s| !s.attributes.writable()) {
            return Err(FileError::io(
                &self.before.path,
                "prepare",
                io::Error::new(io::ErrorKind::PermissionDenied, "file is read-only"),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for FileChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileChange").field("before", &self.before).field("delete", &self.after.is_none()).finish()
    }
}

/// Ownership facts for a successful commit, suitable for the install record.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Created {
    pub files: Vec<PathBuf>,
    pub directories: Vec<CreatedDirectory>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatedDirectory {
    pub path: PathBuf,
    /// None means creation succeeded but reading its identity failed. Cleanup
    /// reports this directory and never guesses that it still belongs to us.
    pub identity: Option<Identity>,
}

#[derive(Debug, Default)]
pub struct Receipt {
    pub changed: Vec<PathBuf>,
    pub created: Created,
    /// Cleanup failures do not turn a fully committed transaction into failure.
    /// In particular, Windows may still be running an old executable image.
    pub cleanup: Vec<FileError>,
    /// Manifests locating retained file objects. These are not disposable
    /// copies: another process can still write through an old open handle.
    pub preserved: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Failure {
    pub cause: Box<FileError>,
    /// Each path that could not be restored, including concurrent modifications.
    pub rollback: Vec<FileError>,
    pub cleanup: Vec<FileError>,
    /// Everything created during the attempt, even if rollback removed it.
    pub created: Created,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cause)?;
        for error in &self.rollback {
            write!(f, "\nrollback: {error}")?;
        }
        for error in &self.cleanup {
            write!(f, "\ncleanup: {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

#[derive(Debug, Default)]
pub struct Transaction {
    changes: Vec<FileChange>,
    cleanup: Vec<CreatedDirectory>,
}

type RenderRecord<'a> = Box<dyn FnOnce(&Created) -> io::Result<Vec<u8>> + 'a>;

impl Transaction {
    pub fn new(changes: impl IntoIterator<Item = FileChange>) -> Self {
        Self { changes: changes.into_iter().collect(), cleanup: vec![] }
    }

    /// Directory cleanup is part of the recoverable outcome, so interruption
    /// after deleting an install record cannot lose these ownership facts.
    pub fn cleanup_after_commit(mut self, directories: &[CreatedDirectory]) -> Self {
        self.cleanup = directories.to_vec();
        self
    }

    pub fn commit(self) -> Result<Receipt, Failure> {
        self.run(None)
    }

    /// Commits with a durable recovery manifest. Call `recover` under the same
    /// installer lock before reading files for a subsequent operation.
    pub fn commit_durable(self, journal: impl AsRef<Path>) -> Result<Receipt, Failure> {
        durable::commit(self.changes, journal.as_ref(), None, self.cleanup)
    }

    pub fn commit_with_record_durable(
        self,
        journal: impl AsRef<Path>,
        record: Snapshot,
        render: impl FnOnce(&Created) -> io::Result<Vec<u8>>,
    ) -> Result<Receipt, Failure> {
        durable::commit(self.changes, journal.as_ref(), Some((record, Box::new(render))), self.cleanup)
    }

    /// Adds the install record last. The renderer runs after parent creation,
    /// before any file publication, and receives the actual ownership facts.
    /// Its error rolls back directory creation as well. The record path must
    /// not also occur in this transaction's changes.
    pub fn commit_with_record(
        self,
        record: Snapshot,
        render: impl FnOnce(&Created) -> io::Result<Vec<u8>>,
    ) -> Result<Receipt, Failure> {
        self.run(Some((record, Box::new(render))))
    }

    fn run(mut self, record: Option<(Snapshot, RenderRecord<'_>)>) -> Result<Receipt, Failure> {
        let render = record.map(|(snapshot, render)| {
            self.changes.push(FileChange::private(snapshot, Vec::new()));
            render
        });
        let mut work = Work::default();
        let result = (|| {
            validate_unique(&self.changes)?;
            for change in &self.changes {
                change.preflight()?;
            }
            for change in &self.changes {
                if change.after.is_some() {
                    work.parents(&change.before)?;
                    if !change.before.exists() {
                        work.created.files.push(change.before.path.clone());
                    }
                }
            }
            if let Some(render) = render {
                let record = self.changes.last_mut().expect("record was appended");
                record.after = Some(
                    render(&work.created)
                        .map_err(|error| FileError::io(&record.before.path, "render record", error))?
                        .into(),
                );
            }
            for change in &self.changes {
                change.preflight()?;
                work.stages.push(prepare(change, false)?);
            }
            for (index, change) in self.changes.iter().enumerate() {
                work.verify_parents()?;
                change.before.verify()?;
                if !change.changes() {
                    continue;
                }
                let (after, result) = publish(change, work.stages[index].as_mut());
                // Publication precedes directory fsync and temporary-name
                // cleanup. Even when those fail, this file must be rolled back.
                if let Some(after) = after {
                    work.committed.push((index, after));
                }
                result?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                let mut cleanup = work.cleanup_stages(&self.changes);
                cleanup.extend(cleanup_directories(&self.cleanup));
                Ok(Receipt {
                    changed: work.committed.iter().map(|(i, _)| self.changes[*i].before.path.clone()).collect(),
                    created: work.created,
                    cleanup,
                    preserved: Vec::new(),
                })
            }
            Err(cause) => {
                let mut rollback = Vec::new();
                for position in (0..work.committed.len()).rev() {
                    let (index, installed) = &work.committed[position];
                    let original = &self.changes[*index];
                    let reverse = FileChange {
                        before: installed.clone(),
                        after: original.before.state.as_ref().map(|s| s.bytes.clone()),
                        mode: original.mode,
                        executable: original.executable,
                    };
                    let result = (|| {
                        work.verify_parents_of(&reverse.before.path)?;
                        reverse.before.verify()?;
                        let mut stage = prepare_restore(&reverse, &original.before)?;
                        let (_, result) = publish(&reverse, stage.as_mut());
                        // Retain cleanup errors even if the restoration itself failed.
                        if let Some(stage) = stage {
                            work.stages.push(Some(stage));
                        }
                        result
                    })();
                    if let Err(error) = result {
                        rollback.push(error);
                    }
                }
                let mut cleanup = work.cleanup_stages(&self.changes);
                cleanup.extend(cleanup_directories(&work.created.directories));
                work.created
                    .files
                    .retain(|path| work.committed.iter().any(|(index, _)| self.changes[*index].before.path == *path));
                Err(Failure { cause: Box::new(cause), rollback, cleanup, created: work.created })
            }
        }
    }
}

#[derive(Default)]
struct Work {
    created: Created,
    parents: Vec<(PathBuf, Identity)>,
    stages: Vec<Option<Prepared>>,
    committed: Vec<(usize, Snapshot)>,
}

impl Work {
    fn parents(&mut self, snapshot: &Snapshot) -> Result<(), FileError> {
        snapshot.verify()?;
        let parent = snapshot.path.parent().expect("absolute path");
        let relative = parent.strip_prefix(&snapshot.anchor.0).expect("resolved beneath anchor");
        let mut path = snapshot.anchor.0.clone();
        self.parents.push((path.clone(), snapshot.anchor.1));
        for part in relative.components() {
            self.verify_parents()?;
            path.push(part);
            let created = match fs::create_dir(&path) {
                Ok(()) => {
                    self.created.directories.push(CreatedDirectory { path: path.clone(), identity: None });
                    true
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(FileError::io(&path, "create directory", error)),
            };
            let identity = directory_identity(&path)?;
            if created {
                self.created.directories.last_mut().expect("just created").identity = Some(identity);
            }
            self.parents.push((path.clone(), identity));
        }
        Ok(())
    }

    fn verify_parents(&self) -> Result<(), FileError> {
        for (path, expected) in &self.parents {
            if directory_identity(path)? != *expected {
                return Err(FileError::changed(path));
            }
        }
        Ok(())
    }

    fn verify_parents_of(&self, target: &Path) -> Result<(), FileError> {
        // An unrelated directory changing must not prevent restoring files
        // whose own paths and installed revisions are still intact.
        for (path, expected) in &self.parents {
            if target.starts_with(path) && directory_identity(path)? != *expected {
                return Err(FileError::changed(path));
            }
        }
        Ok(())
    }

    fn cleanup_stages(&mut self, changes: &[FileChange]) -> Vec<FileError> {
        let mut errors = Vec::new();
        for stage in self.stages.iter_mut().flatten() {
            if let Err(error) = stage.file.discard() {
                errors.push(FileError::io(&stage.file.path, "delete staging file", error));
            }
        }
        let mut directories = HashSet::new();
        for change in changes.iter().filter(|change| change.executable) {
            if directories.insert(change.before.path.parent()) {
                errors.extend(executable::cleanup_retired(&change.before.path));
            }
        }
        errors
    }
}

struct Prepared {
    file: Staged,
    installed: Snapshot,
}

fn prepare(change: &FileChange, restore: bool) -> Result<Option<Prepared>, FileError> {
    prepare_with_attributes(change, &change.before, restore)
}

fn prepare_restore(change: &FileChange, original: &Snapshot) -> Result<Option<Prepared>, FileError> {
    prepare_with_attributes(change, original, true)
}

fn prepare_with_attributes(
    change: &FileChange,
    attributes: &Snapshot,
    restore: bool,
) -> Result<Option<Prepared>, FileError> {
    if !change.changes() {
        return Ok(None);
    }
    let Some(bytes) = &change.after else { return Ok(None) };
    let path = &change.before.path;
    let mode = if attributes.exists() { 0o600 } else { change.mode };
    let mut file = Staged::create(path, mode).map_err(|error| FileError::io(path, "create staging file", error))?;
    let result = (|| {
        if let Some(original) = &attributes.state {
            // Protect the staging name before it contains any user data; the
            // destination's ACL may be narrower than its directory's ACL.
            original.attributes.protect(&file.file)?;
        }
        file.file.write_all(bytes)?;
        if let Some(original) = &attributes.state {
            original.attributes.apply(&file.file, &file.path)?;
            if restore {
                disk::restore_modified(&file.file, original.modified)?;
            }
        }
        file.file.sync_all()?;
        let state = state_of_staged(&file.file, &file.path, bytes.clone())?;
        let installed = Snapshot { state: Some(state), ..change.before.clone() };
        Ok(installed)
    })();
    match result {
        Ok(installed) => Ok(Some(Prepared { file, installed })),
        Err(error) => {
            let error = match file.discard() {
                Ok(()) => error,
                Err(cleanup) => {
                    io::Error::other(format!("{error}; cannot delete staging file {}: {cleanup}", file.path.display()))
                }
            };
            Err(FileError::io(path, "stage file", error))
        }
    }
}

fn publish(change: &FileChange, stage: Option<&mut Prepared>) -> (Option<Snapshot>, Result<(), FileError>) {
    let path = &change.before.path;
    let mut installed = None;
    let result = (|| {
        if let Some(stage) = stage {
            let replace = change.before.exists();
            mutate(change, "publish", || disk::publish(&stage.file.file, &stage.file.path, path, replace))?;
            installed = Some(stage.installed.clone());
            stage.file.present = disk::stage_survives_publication(replace);
            stage.file.discard().map_err(|e| FileError::io(&stage.file.path, "delete staging file", e))?;
        } else {
            let file = disk::open(path).map_err(|e| FileError::io(path, "open for deletion", e))?;
            mutate(change, "delete", || disk::delete(&file, path))?;
            installed = Some(Snapshot { state: None, ..change.before.clone() });
        }
        disk::sync_parent(path).map_err(|e| FileError::io(path, "sync directory", e))
    })();
    (installed, result)
}

fn mutate(change: &FileChange, operation: &'static str, apply: impl Fn() -> io::Result<()>) -> Result<(), FileError> {
    change.before.verify()?;
    let result = match apply() {
        Err(error)
            if cfg!(windows)
                && change.executable
                && change.before.exists()
                && matches!(error.raw_os_error(), Some(5 | 32 | 33)) =>
        {
            // A mapped image cannot lose its last link on Windows. Retention
            // adds a link before retrying the atomic operation; the fixed path
            // is never moved aside. Ordinary files never take this branch.
            executable::retain(&change.before)?;
            change.before.verify()?;
            apply()
        }
        result => result,
    };
    result.map_err(|error| FileError::io(&change.before.path, operation, error))
}

/// Deletes only these recorded directories, only while still the same objects,
/// and only when empty. Errors (including nonempty directories) are returned.
/// Call after the unapply file transaction commits; never recursively delete.
pub fn cleanup_directories(directories: &[CreatedDirectory]) -> Vec<FileError> {
    let mut errors = Vec::new();
    for directory in directories.iter().rev() {
        let result = (|| {
            match fs::symlink_metadata(&directory.path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(FileError::io(&directory.path, "inspect directory", error)),
                Ok(_) => {}
            }
            if Some(directory_identity(&directory.path)?) != directory.identity {
                return Err(FileError::changed(&directory.path));
            }
            fs::remove_dir(&directory.path)
                .map_err(|error| FileError::io(&directory.path, "delete empty directory", error))?;
            disk::sync_parent(&directory.path)
                .map_err(|error| FileError::io(&directory.path, "sync directory deletion", error))
        })();
        if let Err(error) = result {
            errors.push(error);
        }
    }
    errors
}

fn directory_identity(path: &Path) -> Result<Identity, FileError> {
    let result = (|| {
        let file = disk::open_directory(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || disk::is_link(&metadata) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "parent is not an ordinary directory"));
        }
        disk::identity(&file)
    })();
    result.map_err(|error| FileError::io(path, "inspect directory", error))
}

fn validate_unique(changes: &[FileChange]) -> Result<(), FileError> {
    let mut paths = HashSet::new();
    let mut identities = Vec::new();
    for change in changes {
        let path = &change.before.path;
        let key = if cfg!(windows) { path.as_os_str().to_ascii_lowercase() } else { path.as_os_str().to_owned() };
        let duplicate =
            !paths.insert(key) || change.before.state.as_ref().is_some_and(|s| identities.contains(&s.identity));
        if duplicate {
            return Err(FileError {
                path: path.clone(),
                operation: "prepare",
                problem: Problem::Invalid("duplicate target or hard-link alias in transaction"),
            });
        }
        if let Some(state) = &change.before.state {
            identities.push(state.identity);
        }
    }
    Ok(())
}
