//! Path policy, cooperating-writer locks, snapshots, and atomic publication.

use super::{ChangeKind, FileChange, WriteContext, content_hash};
use crate::edit::locks::{FilePathLock, PathIdentity};
use crate::paths::{canonical_existing, display_path, io_error_message};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) struct Target {
    pub requested: PathBuf,
    pub resolved: PathBuf,
}

impl Target {
    pub fn new(path: &Path, context: &WriteContext<'_>) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err(format!(
                "Use an absolute path, not a relative path: {}",
                display_path(path)
            ));
        }
        check_directory(path, context)?;
        if let Some(parent) = path.parent() {
            let resolved_parent = resolve(parent)?;
            check_directory(
                &resolved_parent.join(path.file_name().unwrap_or_default()),
                context,
            )?;
        }
        let resolved = resolve(path)?;
        check_directory(&resolved, context)?;
        Ok(Self {
            requested: path.to_owned(),
            resolved,
        })
    }

    pub fn create_parent(&self, context: &WriteContext<'_>) -> Result<(), String> {
        self.validate(context)?;
        let parent = self
            .resolved
            .parent()
            .ok_or_else(|| "File path has no parent directory.".to_string())?;
        fs::create_dir_all(parent).map_err(|error| io_error_message(parent, &error))
    }

    pub fn validate(&self, context: &WriteContext<'_>) -> Result<(), String> {
        let current = Self::new(&self.requested, context)?;
        if current.resolved != self.resolved {
            return Err(format!(
                "{} changed its symlink target during the write; retry against the current file.",
                display_path(&self.requested)
            ));
        }
        Ok(())
    }

    pub fn read(&self) -> Result<Option<Vec<u8>>, String> {
        match fs::read(&self.resolved) {
            Ok(raw) => Ok(Some(raw)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error_message(&self.requested, &error)),
        }
    }

    pub fn read_required(&self) -> Result<Vec<u8>, String> {
        self.read()?
            .ok_or_else(|| format!("File does not exist: {}", display_path(&self.requested)))
    }

    pub fn verify(&self, before: Option<&[u8]>, context: &WriteContext<'_>) -> Result<(), String> {
        self.validate(context)?;
        if self.read()?.as_deref() != before {
            return Err(format!(
                "{} changed on disk during the write; this file was not written. Read it and retry.",
                display_path(&self.requested)
            ));
        }
        Ok(())
    }

    pub fn publish(
        &self,
        before: Option<&[u8]>,
        after: &[u8],
        context: &WriteContext<'_>,
    ) -> Result<FileChange, String> {
        self.verify(before, context)?;
        crate::atomic_file::publish(&self.resolved, after)?;
        Ok(FileChange {
            file_path: self.requested.clone(),
            kind: if before.is_some() {
                ChangeKind::Modified
            } else {
                ChangeKind::Added
            },
            before_hash: before.map(content_hash),
            after_hash: Some(content_hash(after)),
        })
    }

    pub fn delete(&self, before: &[u8], context: &WriteContext<'_>) -> Result<FileChange, String> {
        self.verify(Some(before), context)?;
        fs::remove_file(&self.requested)
            .map_err(|error| io_error_message(&self.requested, &error))?;
        Ok(FileChange {
            file_path: self.requested.clone(),
            kind: ChangeKind::Deleted,
            before_hash: Some(content_hash(before)),
            after_hash: None,
        })
    }
}

fn check_directory(path: &Path, context: &WriteContext<'_>) -> Result<(), String> {
    if let Some(denied) = context.denied_directory {
        for directory in path.ancestors().skip(1) {
            if denied(directory) {
                return Err(format!(
                    "Writing under {} is refused by the directory policy: {}",
                    display_path(directory),
                    display_path(path)
                ));
            }
        }
    }
    Ok(())
}

/// Resolve existing ancestors as well as final symlinks before checking directory policy.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    match canonical_existing(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                return Err(format!(
                    "Cannot write through a dangling symlink: {}",
                    display_path(path)
                ));
            }
            let parent = path
                .parent()
                .ok_or_else(|| io_error_message(path, &error))?;
            let parent = resolve(parent)?;
            let name = path
                .file_name()
                .ok_or_else(|| io_error_message(path, &error))?;
            Ok(parent.join(name))
        }
        Err(error) => Err(io_error_message(path, &error)),
    }
}

/// Name locks precede identity locks, matching the 1.0 replacement lock order.
pub(super) fn lock(targets: &[&Target]) -> Result<Vec<FilePathLock>, String> {
    let mut names = BTreeMap::new();
    for target in targets {
        let mut path = target.resolved.as_path();
        while path.parent().is_some_and(|parent| !parent.exists()) {
            path = path.parent().expect("parent checked above");
        }
        names.insert(PathIdentity::for_name(path)?, path);
    }
    let mut guards = Vec::new();
    for (identity, path) in names {
        guards.push(FilePathLock::acquire(&identity, path)?);
    }
    let mut identities = BTreeMap::new();
    for target in targets {
        match fs::metadata(&target.resolved) {
            Ok(_) => {
                identities.insert(
                    PathIdentity::for_existing(&target.resolved)?,
                    &target.resolved,
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error_message(&target.requested, &error)),
        }
    }
    for (identity, path) in identities {
        guards.push(FilePathLock::acquire(&identity, path)?);
    }
    Ok(guards)
}
