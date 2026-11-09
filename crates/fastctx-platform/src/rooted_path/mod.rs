//! Directory-handle capabilities for managed working copies.
//!
//! Names are opened one component at a time without following links/reparse
//! points. Mutations use the held parent, never a canonicalize-then-open path.
//! Callers serialize managed namespace changes; a handle denotes a directory
//! identity, even if another process with the same OS privileges moves it.

mod entries;
mod names;
pub use entries::Entries;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub(crate) mod windows;
#[cfg(windows)]
mod windows_security;
pub use names::{Name, NameMapping, map_name, validate_windows};
use std::{
    fs::File,
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
use windows as imp;

pub const MAX_DEPTH: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    pub volume: u64,
    pub file: u64,
}
#[derive(Clone)]
pub struct Directory(Arc<Node>);
struct Node {
    file: File,
    identity: FileIdentity,
    parent: Option<Directory>,
    name: Option<Name>,
    path: PathBuf,
    depth: usize,
}
impl std::fmt::Debug for Directory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Directory").field("identity", &self.0.identity).finish_non_exhaustive()
    }
}
impl Directory {
    /// Opens existing ancestors without following links; only the final managed
    /// root may be created. Existing roots must already be owner-private.
    pub fn open_root(path: &Path, create: bool) -> io::Result<Self> {
        crate::roles::in_place("open managed root", || {
            if !path.is_absolute() {
                return Err(invalid("managed root must be absolute"));
            }
            let (base, components) = imp::absolute_components(path)?;
            if components.is_empty() || components.len() > MAX_DEPTH {
                return Err(invalid("invalid managed root depth"));
            }
            let file = imp::open_anchor(&base)?;
            let mut current = Self(Arc::new(Node {
                identity: identity(&file)?,
                file,
                parent: None,
                name: None,
                path: base,
                depth: 0,
            }));
            for (index, name) in components.iter().enumerate() {
                current = current.child(name, create && index + 1 == components.len())?;
            }
            imp::check_private(current.file())?;
            current.sync_chain()?;
            Ok(current)
        })
    }
    fn child(&self, name: &Name, create: bool) -> io::Result<Self> {
        if self.0.depth >= MAX_DEPTH {
            return Err(invalid("managed path exceeds depth bound"));
        }
        let file = imp::open_directory(self.file(), name, create)?;
        let result = Self(Arc::new(Node {
            identity: identity(&file)?,
            file,
            parent: Some(self.clone()),
            name: Some(name.clone()),
            path: self.0.path.join(name.as_os_str()),
            depth: self.0.depth + 1,
        }));
        if create {
            imp::check_private(result.file())?;
            result.sync_chain()?;
        }
        Ok(result)
    }
    pub fn open_directory(&self, name: &Name) -> io::Result<Self> {
        crate::roles::in_place("open managed directory", || self.child(name, false))
    }
    pub fn create_directory(&self, name: &Name) -> io::Result<Self> {
        crate::roles::in_place("create managed directory", || self.child(name, true))
    }
    pub fn descend(&self, relative: &Path) -> io::Result<Self> {
        let mut current = self.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(invalid("expected relative components"));
            };
            current = current.open_directory(&Name::new(name)?)?;
        }
        Ok(current)
    }
    pub fn identity(&self) -> FileIdentity {
        self.0.identity
    }
    /// Display only; reopening this string would discard the capability.
    pub fn display_path(&self) -> &Path {
        &self.0.path
    }
    pub(crate) fn file(&self) -> &File {
        &self.0.file
    }
    pub fn verify_attached(&self) -> io::Result<()> {
        let mut current = Some(self);
        while let Some(dir) = current {
            if identity(dir.file())? != dir.0.identity {
                return Err(invalid("managed directory identity changed"));
            }
            if let (Some(parent), Some(name)) = (&dir.0.parent, &dir.0.name) {
                let actual = imp::open_directory(parent.file(), name, false)?;
                if identity(&actual)? != dir.0.identity {
                    return Err(invalid("managed path was replaced"));
                }
            } else {
                let actual = imp::open_anchor(&dir.0.path)?;
                if identity(&actual)? != dir.0.identity {
                    return Err(invalid("managed volume anchor was replaced"));
                }
            }
            current = dir.0.parent.as_ref();
        }
        Ok(())
    }
    /// Unix fsyncs every held ancestor, including ones that already existed,
    /// without a string-path cache. Windows creates with write-through but has
    /// no ordinary-directory flush contract; startup reconciliation remains
    /// mandatory even after a successful installed checkpoint.
    pub fn sync_chain(&self) -> io::Result<()> {
        crate::roles::in_place("sync managed namespace", || {
            let mut current = Some(self);
            while let Some(dir) = current {
                imp::sync_directory(dir.file())?;
                current = dir.0.parent.as_ref();
            }
            Ok(())
        })
    }
    /// Streams names from this directory identity with bounded buffering. Does
    /// not follow entries; open each child through this same capability. Two
    /// iterators have independent cursors, including across a Unix exchange.
    pub fn entries(&self) -> io::Result<Entries> {
        crate::roles::in_place("open managed directory cursor", || {
            #[cfg(unix)]
            let file = self.file().try_clone()?;
            #[cfg(windows)]
            let file = match (&self.0.parent, &self.0.name) {
                (Some(parent), Some(name)) => imp::open_directory(parent.file(), name, false)?,
                _ => imp::open_anchor(&self.0.path)?,
            };
            if identity(&file)? != self.identity() {
                return Err(invalid("enumeration identity changed"));
            }
            Entries::open(file)
        })
    }
    pub fn open_file(&self, name: &Name) -> io::Result<File> {
        crate::roles::in_place("open managed file", || imp::open_file(self.file(), name, false, false))
    }
    pub(crate) fn open_prepared(&self, name: &Name) -> io::Result<File> {
        imp::open_prepared(self.file(), name)
    }
    pub(crate) fn confirm_prepared(&self, file: &File) -> io::Result<()> {
        imp::confirm_prepared(file)
    }
    pub(crate) fn create_file(&self, name: &Name) -> io::Result<File> {
        imp::open_file(self.file(), name, true, true)
    }
    pub(crate) fn resume_file(&self, name: &Name, expected: FileIdentity) -> io::Result<File> {
        match imp::open_file(self.file(), name, false, true) {
            Ok(file) => Ok(file),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                // A crash can follow the final mode change but precede the
                // Prepared checkpoint. Only that intent's exact stage may be
                // made writable again; accepted visible files are not touched.
                let file = imp::open_prepared(self.file(), name)?;
                if identity(&file)? != expected {
                    return Err(invalid("stage identity changed"));
                }
                imp::set_mode(&file, 0o600)?;
                drop(file);
                imp::open_file(self.file(), name, false, true)
            }
            Err(error) => Err(error),
        }
    }
    pub fn file_identity(&self, name: &Name) -> io::Result<Option<FileIdentity>> {
        match self.open_file(name) {
            Ok(file) => Ok(Some(identity(&file)?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub(crate) fn rename_file(&self, from: &Name, file: &File, to: &Name, replace: bool) -> io::Result<()> {
        imp::rename_file(self.file(), from, file, to, replace)
    }
    pub(crate) fn set_mode(file: &File, mode: u32) -> io::Result<()> {
        imp::set_mode(file, mode)
    }
    #[cfg(unix)]
    pub(crate) fn exchange(&self, left: &Name, right: &Name) -> io::Result<()> {
        imp::exchange(self.file(), left, right)
    }
    #[cfg(unix)]
    pub(crate) fn install_directory(&self, from: &Name, to: &Name) -> io::Result<()> {
        imp::install_directory(self.file(), from, to)
    }
    pub(crate) fn verify_private(&self) -> io::Result<()> {
        imp::check_private(self.file())
    }
}
pub fn identity(file: &File) -> io::Result<FileIdentity> {
    imp::identity(file)
}
pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}
