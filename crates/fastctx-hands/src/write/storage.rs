use super::{ChangeKind, FileChange, Writer};
use crate::files::{canonical_path, content_hash};
use fastctx_platform::atomic::{Permissions, publish_checked};
use fastctx_types::files::{SeenFile, SeenFiles, SeenVersion};
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(super) struct PublishedEncoding<'a> {
    pub before_selection: Option<&'a str>,
    pub label: Option<&'a str>,
    pub selection: Option<&'a str>,
}

pub(super) struct Target {
    pub requested: PathBuf,
    pub resolved: PathBuf,
}
pub(super) struct Snapshot {
    pub bytes: Vec<u8>,
    pub metadata: Metadata,
}

impl Snapshot {
    fn matches(&self, seen: &SeenFile) -> io::Result<bool> {
        Ok(match &seen.version {
            SeenVersion::Full { sha256 } => *sha256 == content_hash(&self.bytes),
            SeenVersion::Partial { size, modified } => {
                *size == self.metadata.len() && *modified == self.metadata.modified()?
            }
        })
    }
}

impl Target {
    pub fn new(path: &Path, writer: &Writer) -> Result<Self, String> {
        let native = fastctx_platform::local_path(path).map_err(|e| e.to_string())?;
        let path = native.as_path();
        if !path.is_absolute() {
            return Err(format!(
                "{} is a relative path; the World's file commands take only absolute paths, as /… or C:/… on Windows",
                path.display()
            ));
        }
        check_policy(path, writer)?;
        let resolved = canonical_path(path).map_err(|e| error(path, e))?;
        check_policy(&resolved, writer)?;
        Ok(Self { requested: path.to_path_buf(), resolved })
    }

    pub fn create_parent(&self, writer: &Writer) -> Result<(), String> {
        self.validate(writer)?;
        let parent = self.resolved.parent().ok_or_else(|| "File path has no parent".to_string())?;
        fs::create_dir_all(parent).map_err(|e| error(parent, e))
    }

    fn validate(&self, writer: &Writer) -> Result<(), String> {
        let current = Self::new(&self.requested, writer)?;
        if current.resolved != self.resolved {
            return Err(format!(
                "{} changed its link target during the write; view it again",
                self.requested.display()
            ));
        }
        Ok(())
    }

    pub fn read(&self) -> Result<Option<Snapshot>, String> {
        let metadata = match fs::metadata(&self.resolved) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(error(&self.requested, e)),
        };
        if !metadata.is_file() {
            return Err(format!("Not a regular file: {}", self.requested.display()));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A file replaced by a FIFO between stat and open must not block the node.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let mut file = options.open(&self.resolved).map_err(|e| error(&self.requested, e))?;
        let metadata = file.metadata().map_err(|e| error(&self.requested, e))?;
        if !metadata.is_file() {
            return Err(format!("Not a regular file: {}", self.requested.display()));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| error(&self.requested, e))?;
        let after = file.metadata().map_err(|e| error(&self.requested, e))?;
        if metadata.len() != after.len()
            || metadata.modified().map_err(|e| error(&self.requested, e))?
                != after.modified().map_err(|e| error(&self.requested, e))?
        {
            return Err(format!("{} changed while being read; view it again", self.requested.display()));
        }
        Ok(Some(Snapshot { bytes, metadata }))
    }

    pub fn required(&self) -> Result<Snapshot, String> {
        self.read()?.ok_or_else(|| format!("File does not exist: {}", self.requested.display()))
    }

    pub fn observation<'a>(&self, seen: &'a SeenFiles) -> Option<&'a SeenFile> {
        seen.get(&self.resolved).or_else(|| seen.get(&self.requested))
    }

    pub fn stale(&self, snapshot: &Snapshot, seen: &SeenFiles) -> Result<bool, String> {
        self.observation(seen)
            .map(|seen| snapshot.matches(seen).map(|same| !same))
            .transpose()
            .map(|stale| stale.unwrap_or(false))
            .map_err(|e| error(&self.requested, e))
    }

    pub fn encoding<'a>(&self, snapshot: &Snapshot, seen: &'a SeenFiles) -> Result<Option<&'a str>, String> {
        if self.stale(snapshot, seen)? {
            return Ok(None);
        }
        Ok(self.observation(seen).and_then(|entry| entry.encoding.as_deref()))
    }

    pub fn whole_overwrite(&self, before: &Snapshot, seen: &SeenFiles) -> Result<(), String> {
        if self.stale(before, seen)? {
            return Err(format!(
                "{} changed since you last viewed it. Whole-file overwrite refused; use fastctx view before writing again",
                self.requested.display()
            ));
        }
        Ok(())
    }

    pub fn verify(&self, before: Option<&Snapshot>, writer: &Writer) -> Result<(), String> {
        self.validate(writer)?;
        let now = self.read()?;
        if now.as_ref().map(|s| &s.bytes) != before.map(|s| &s.bytes) {
            return Err(format!(
                "{} changed on disk during the write; this file was not written. View it and retry",
                self.requested.display()
            ));
        }
        Ok(())
    }

    pub fn publish(
        &self,
        before: Option<&Snapshot>,
        after: &[u8],
        encoding: PublishedEncoding<'_>,
        writer: &Writer,
        changes: &mut Vec<FileChange>,
    ) -> Result<(), String> {
        let fact = writer
            .capture
            .as_ref()
            .map(|capture| {
                capture.prepare(
                    &self.requested,
                    before.map(|snapshot| snapshot.bytes.as_slice()),
                    Some(after),
                    encoding.before_selection,
                    encoding.selection,
                    changes.len(),
                )
            })
            .transpose()?;
        let result = publish_checked(&self.resolved, after, Permissions::KeepExisting, before.is_some(), || {
            self.verify(before, writer).map_err(io::Error::other)
        });
        if result.as_ref().map_or_else(|failure| failure.published, |_| true) {
            changes.push(FileChange {
                fact,
                path: self.requested.clone(),
                kind: if before.is_some() { ChangeKind::Modified } else { ChangeKind::Added },
                before_hash: before.map(|s| content_hash(&s.bytes)),
                after_hash: Some(content_hash(after)),
                encoding: encoding.label.map(str::to_owned),
                encoding_selection: encoding.selection.map(str::to_owned),
            });
        } else if let Some(diff) = fact.and_then(|fact| fact.diff) {
            fs::remove_file(diff.path).map_err(|error| error.to_string())?;
        }
        result.map_err(|failure| {
            if failure.published {
                format!("{} was published, but durability failed: {}", self.requested.display(), failure.error)
            } else {
                error(&self.requested, failure.error)
            }
        })
    }

    pub fn delete(
        &self,
        before: &Snapshot,
        selection: Option<&str>,
        writer: &Writer,
        changes: &mut Vec<FileChange>,
    ) -> Result<(), String> {
        let fact = writer
            .capture
            .as_ref()
            .map(|capture| capture.prepare(&self.requested, Some(&before.bytes), None, selection, None, changes.len()))
            .transpose()?;
        let result = self
            .verify(Some(before), writer)
            .and_then(|()| fs::remove_file(&self.requested).map_err(|e| error(&self.requested, e)));
        if let Err(error) = result {
            if let Some(diff) = fact.and_then(|fact| fact.diff) {
                fs::remove_file(diff.path).map_err(|error| error.to_string())?;
            }
            return Err(error);
        }
        changes.push(FileChange {
            fact,
            path: self.requested.clone(),
            kind: ChangeKind::Deleted,
            before_hash: Some(content_hash(&before.bytes)),
            after_hash: None,
            encoding: None,
            encoding_selection: None,
        });
        #[cfg(unix)]
        {
            let parent = self.requested.parent().expect("absolute file has parent");
            std::fs::File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|e| format!("{} was deleted, but durability failed: {e}", self.requested.display()))?;
        }
        Ok(())
    }
}

fn error(path: &Path, e: io::Error) -> String {
    let written = path.display().to_string();
    if fastctx_platform::privacy::refused(&written, &e) {
        return fastctx_platform::privacy::said(&written);
    }
    format!("{written}: {e}")
}

const LEDGER: &str =
    "Project ledger files and memory copies cannot be modified directly; use write or edit for project memory";

fn check_policy(path: &Path, writer: &Writer) -> Result<(), String> {
    let towns = writer.world.join("towns");
    if relative_parts(path, &writer.world.join("write.lock")).is_some_and(|parts| parts.is_empty()) {
        // Replacing the lock inode would split cooperating writers into two groups.
        return Err("fastctx's write lock is in use and cannot be replaced".into());
    }
    // A town's ledger, and the signed entries it is made of, change only by
    // taking entries. Check the lexical path even before a town has them,
    // and the resolved ones when a town or its ledger is a directory link.
    let shared = ["ledger", "signed", "journals", "specs"].map(std::ffi::OsStr::new);
    if let Some(parts) = relative_parts(path, &towns)
        && parts.len() >= 2
        && shared.iter().any(|name| component_eq(parts[1], name))
    {
        return Err(LEDGER.into());
    }
    match fs::read_dir(&towns) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|e| error(&towns, e))?;
                for name in shared {
                    let root = canonical_path(&entry.path().join(name)).map_err(|e| error(path, e))?;
                    if relative_parts(path, &root).is_some() {
                        return Err(LEDGER.into());
                    }
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(error(&towns, e)),
    }
    Ok(())
}

fn component_eq(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

fn relative_parts<'a>(path: &'a Path, root: &Path) -> Option<Vec<&'a std::ffi::OsStr>> {
    let mut path = path.components();
    for component in root.components() {
        if !component_eq(path.next()?.as_os_str(), component.as_os_str()) {
            return None;
        }
    }
    Some(path.map(|part| part.as_os_str()).collect())
}
