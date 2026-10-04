use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::disk::{self, Attributes, Identity};
use crate::transaction::{FileError, Problem};

/// The bytes, identity, modification time and attributes observed together.
/// Read this before computing edits; constructing a new snapshot after editing
/// would defeat conflict detection. Final symlinks and special files are refused.
/// Debug output deliberately excludes file contents.
#[derive(Clone)]
pub struct Snapshot {
    pub(crate) path: PathBuf,
    pub(crate) anchor: (PathBuf, Identity),
    pub(crate) state: Option<State>,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot").field("path", &self.path).field("exists", &self.state.is_some()).finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct State {
    pub bytes: Arc<[u8]>,
    pub identity: Identity,
    pub modified: SystemTime,
    pub attributes: Attributes,
}

impl Snapshot {
    /// Resolves the existing parent once. Subsequent operations use that fixed
    /// absolute path and check the parent's identity before touching its files.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, FileError> {
        let requested = path.as_ref();
        let (path, anchor) = resolve(requested).map_err(|error| FileError::io(requested, "read parent", error))?;
        let mut snapshot = Self { path, anchor, state: None };
        snapshot.state = snapshot.current()?;
        Ok(snapshot)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        self.state.as_ref().map(|state| state.bytes.as_ref())
    }

    pub fn exists(&self) -> bool {
        self.state.is_some()
    }

    pub(crate) fn verify(&self) -> Result<(), FileError> {
        if self.current()? != self.state {
            return Err(FileError::changed(&self.path));
        }
        Ok(())
    }

    pub(crate) fn current(&self) -> Result<Option<State>, FileError> {
        let anchor = disk::open_directory(&self.anchor.0)
            .and_then(|file| disk::identity(&file))
            .map_err(|error| FileError::io(&self.anchor.0, "verify parent", error))?;
        if anchor != self.anchor.1 {
            return Err(FileError::changed(&self.anchor.0));
        }
        read_state(&self.path).map_err(|problem| FileError { path: self.path.clone(), operation: "read", problem })
    }
}

pub(crate) fn read_state(path: &Path) -> Result<Option<State>, Problem> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Problem::Io(error)),
    };
    disk::ordinary(&metadata)?;
    let mut file = disk::open(path)?;
    disk::ordinary(&file.metadata()?)?;
    let identity = disk::identity(&file)?;
    let modified = file.metadata()?.modified()?;
    let attributes = Attributes::read(&file, path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    // The two metadata observations detect writes during the read. The second
    // handle also detects a path replacement while the first handle was open.
    let current = disk::open(path)?;
    if disk::identity(&current)? != identity
        || current.metadata()?.modified()? != modified
        || Attributes::read(&current, path)? != attributes
        || current.metadata()?.len() != bytes.len() as u64
    {
        return Err(Problem::Changed);
    }
    Ok(Some(State { bytes: bytes.into(), identity, modified, attributes }))
}

pub(crate) fn state_of_staged(file: &File, path: &Path, bytes: Arc<[u8]>) -> io::Result<State> {
    Ok(State {
        bytes,
        identity: disk::identity(file)?,
        modified: file.metadata()?.modified()?,
        attributes: Attributes::read(file, path)?,
    })
}

pub(crate) fn resolve(path: &Path) -> io::Result<(PathBuf, (PathBuf, Identity))> {
    let absolute = std::path::absolute(path)?;
    if absolute.components().any(|c| c == Component::ParentDir) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "file path must not contain '..'"));
    }
    let name = absolute.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path names no file"))?;
    if cfg!(windows) && name.to_string_lossy().contains(':') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "file path names an alternate stream"));
    }
    let mut parent = absolute.parent().expect("absolute file path");
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(parent) {
            Ok(mut canonical) => {
                let file = disk::open_directory(&canonical)?;
                if !file.metadata()?.is_dir() {
                    return Err(io::Error::new(io::ErrorKind::NotADirectory, "parent is not a directory"));
                }
                let anchor = (canonical.clone(), disk::identity(&file)?);
                for part in missing.iter().rev() {
                    canonical.push(part);
                }
                canonical.push(name);
                return Ok((canonical, anchor));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::symlink_metadata(parent).is_ok() {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "parent is a dangling link"));
                }
                missing.push(parent.file_name().ok_or(error)?);
                parent = parent.parent().expect("missing non-root parent");
            }
            Err(error) => return Err(error),
        }
    }
}
