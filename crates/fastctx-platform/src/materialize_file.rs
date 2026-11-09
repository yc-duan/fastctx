//! Recoverable, checked streaming publication below a managed directory.
//!
//! The synchronous journal is supplied by the registered Runtime store. It must
//! durably commit before returning and preserve operation identity on retry.
//! This module creates no sidecar journal, deletes no asset, and retains failed
//! stages. The content checker verifies the accepted root using bounded IO.

use crate::rooted_path::{Directory, FileIdentity, Name, identity};
use fastctx_types::sync::identity::{Digest, OperationId};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Unchanged,
    Published,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Busy,
    DiskFull,
    Denied,
    Conflict,
    Invalid,
    Unsupported,
    Io,
}
#[derive(Debug)]
pub struct Failure {
    pub error: io::Error,
    pub kind: FailureKind,
    pub visibility: Visibility,
}
impl Failure {
    pub(crate) fn new(error: io::Error, visibility: Visibility) -> Self {
        let kind = match error.kind() {
            io::ErrorKind::StorageFull => FailureKind::DiskFull,
            io::ErrorKind::PermissionDenied => FailureKind::Denied,
            io::ErrorKind::AlreadyExists => FailureKind::Conflict,
            io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData => FailureKind::Invalid,
            io::ErrorKind::Unsupported => FailureKind::Unsupported,
            _ => FailureKind::Io,
        };
        #[cfg(windows)]
        let kind = match error.raw_os_error() {
            Some(32 | 33) => FailureKind::Busy,
            Some(39 | 112) => FailureKind::DiskFull,
            _ => kind,
        };
        #[cfg(unix)]
        let kind = match error.raw_os_error() {
            Some(libc::ENOSPC | libc::EDQUOT) => FailureKind::DiskFull,
            Some(libc::EBUSY) => FailureKind::Busy,
            _ => kind,
        };
        Self { error, kind, visibility }
    }
    pub(crate) fn unchanged(error: io::Error) -> Self {
        Self::new(error, Visibility::Unchanged)
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} ({:?}): {}", self.kind, self.visibility, self.error)
    }
}
impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMode {
    pub read_only: bool,
    pub executable: bool,
}
impl FileMode {
    fn bits(self) -> u32 {
        0o400 | if self.read_only { 0 } else { 0o200 } | if self.executable { 0o100 } else { 0 }
    }
    fn matches(self, file: &File) -> io::Result<bool> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok(file.metadata()?.permissions().mode() & 0o777 == self.bits())
        }
        #[cfg(windows)]
        {
            Ok(file.metadata()?.permissions().readonly() == self.read_only)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePlan {
    pub operation: OperationId,
    pub parent: FileIdentity,
    pub stage: Name,
    pub target: Name,
    /// None requires an absent target, never an unconditional overwrite.
    pub expected: Option<FileIdentity>,
    pub bytes: u64,
    /// The expected content root; interpretation belongs to the content checker.
    pub content: Digest,
    pub mode: FileMode,
}
impl FilePlan {
    fn validate(&self, parent: &Directory) -> io::Result<()> {
        if self.parent != parent.identity() || self.stage.could_alias(&self.target) {
            return Err(invalid("file intent does not match its managed parent"));
        }
        parent.verify_attached()?;
        parent.verify_private()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileCheckpoint {
    Intent,
    StageCreated { identity: FileIdentity },
    Prepared { identity: FileIdentity },
    Installed { identity: FileIdentity },
}
/// IO boundaries for nonpersistent tracing and process-crash health probes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileBoundary {
    StageCreated,
    StageSynced,
    StageNamespaceSynced,
    ContentChecked,
    ModeApplied,
    ContentSynced,
    PreparedNamespaceSynced,
    NamespaceChanged,
    PublishedSynced,
    PublishedNamespaceSynced,
}
/// No callback may publish before its transaction is FULL committed. Repeated
/// records must compare the same intent and never regress a committed phase.
/// The caller serializes this binding with wanted, cleanup and revocation.
pub trait FileJournal {
    fn record(&mut self, plan: &FilePlan, checkpoint: FileCheckpoint) -> io::Result<()>;
    /// Observations do not acknowledge durability or advance Runtime state.
    fn observed(&mut self, _plan: &FilePlan, _boundary: FileBoundary) {}
}

pub struct StagedFile {
    parent: Directory,
    plan: FilePlan,
    file: File,
    written: u64,
    failed: bool,
}
impl StagedFile {
    pub fn create(parent: Directory, plan: FilePlan, journal: &mut impl FileJournal) -> Result<Self, Failure> {
        crate::roles::in_place("create materialization stage", || {
            plan.validate(&parent).map_err(Failure::unchanged)?;
            journal.record(&plan, FileCheckpoint::Intent).map_err(Failure::unchanged)?;
            let file = parent.create_file(&plan.stage).map_err(Failure::unchanged)?;
            journal.observed(&plan, FileBoundary::StageCreated);
            file.sync_all().map_err(Failure::unchanged)?;
            journal.observed(&plan, FileBoundary::StageSynced);
            parent.sync_chain().map_err(Failure::unchanged)?;
            journal.observed(&plan, FileBoundary::StageNamespaceSynced);
            journal
                .record(&plan, FileCheckpoint::StageCreated { identity: identity(&file).map_err(Failure::unchanged)? })
                .map_err(Failure::unchanged)?;
            Ok(Self { parent, plan, file, written: 0, failed: false })
        })
    }
    /// The verifier returns only a proven prefix; an unproven tail is truncated.
    /// No optimistic file length is ever treated as a trusted resume position.
    pub fn resume(
        parent: Directory,
        plan: FilePlan,
        expected: FileIdentity,
        verify_prefix: impl FnOnce(&mut File) -> io::Result<u64>,
    ) -> Result<Self, Failure> {
        crate::roles::in_place("resume materialization stage", || {
            plan.validate(&parent).map_err(Failure::unchanged)?;
            let mut file = parent.resume_file(&plan.stage, expected).map_err(Failure::unchanged)?;
            if identity(&file).map_err(Failure::unchanged)? != expected {
                return Err(Failure::unchanged(invalid("stage identity changed")));
            }
            let prefix = verify_prefix(&mut file).map_err(Failure::unchanged)?;
            if prefix > plan.bytes || prefix > file.metadata().map_err(Failure::unchanged)?.len() {
                return Err(Failure::unchanged(invalid("invalid verified prefix")));
            }
            file.set_len(prefix).map_err(Failure::unchanged)?;
            file.seek(SeekFrom::Start(prefix)).map_err(Failure::unchanged)?;
            Ok(Self { parent, plan, file, written: prefix, failed: false })
        })
    }
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        crate::roles::in_place("write materialization stage", || {
            if self.failed || self.written.checked_add(bytes.len() as u64).is_none_or(|n| n > self.plan.bytes) {
                return Err(Failure::unchanged(invalid("stage exceeds expected length or requires recovery")));
            }
            if let Err(error) = self.file.write_all(bytes) {
                self.failed = true;
                return Err(Failure::unchanged(error));
            }
            self.written += bytes.len() as u64;
            Ok(())
        })
    }
    /// Uses one fixed buffer, independent of asset size. The caller may instead
    /// write already verified blocks with write_all.
    pub fn copy_from(&mut self, source: &mut impl Read) -> Result<(), Failure> {
        crate::roles::in_place("stream materialization content", || {
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = source.read(&mut buffer).map_err(Failure::unchanged)?;
                if count == 0 {
                    return Ok(());
                }
                self.write_all(&buffer[..count])?;
            }
        })
    }
    pub fn prepare(
        mut self,
        journal: &mut impl FileJournal,
        check_content: impl FnOnce(&mut File, &FilePlan) -> io::Result<()>,
    ) -> Result<PreparedFile, Failure> {
        crate::roles::in_place("prepare materialized file", || {
            if self.failed
                || self.written != self.plan.bytes
                || self.file.metadata().map_err(Failure::unchanged)?.len() != self.plan.bytes
            {
                return Err(Failure::unchanged(invalid("staged length is incomplete")));
            }
            self.file.seek(SeekFrom::Start(0)).map_err(Failure::unchanged)?;
            check_content(&mut self.file, &self.plan).map_err(Failure::unchanged)?;
            journal.observed(&self.plan, FileBoundary::ContentChecked);
            Directory::set_mode(&self.file, self.plan.mode.bits()).map_err(Failure::unchanged)?;
            journal.observed(&self.plan, FileBoundary::ModeApplied);
            self.file.sync_all().map_err(Failure::unchanged)?;
            journal.observed(&self.plan, FileBoundary::ContentSynced);
            self.parent.sync_chain().map_err(Failure::unchanged)?;
            journal.observed(&self.plan, FileBoundary::PreparedNamespaceSynced);
            let id = identity(&self.file).map_err(Failure::unchanged)?;
            journal.record(&self.plan, FileCheckpoint::Prepared { identity: id }).map_err(Failure::unchanged)?;
            Ok(PreparedFile { parent: self.parent, plan: self.plan, file: self.file, identity: id })
        })
    }
}

pub struct PreparedFile {
    parent: Directory,
    plan: FilePlan,
    file: File,
    identity: FileIdentity,
}
#[derive(Clone, Debug)]
pub struct PublishedFile {
    pub identity: FileIdentity,
    pub operation: OperationId,
}
impl PreparedFile {
    /// The returned guard stays held through rename, namespace confirmation and
    /// installed commit. It fences wanted/holding/exposure, not just byte hashes.
    pub fn publish<G>(
        self,
        journal: &mut impl FileJournal,
        verify: impl FnOnce() -> io::Result<G>,
    ) -> Result<PublishedFile, Failure> {
        crate::roles::in_place("publish materialized file", || {
            let _guard = verify().map_err(Failure::unchanged)?;
            self.plan.validate(&self.parent).map_err(Failure::unchanged)?;
            if self.parent.file_identity(&self.plan.stage).map_err(Failure::unchanged)? != Some(self.identity) {
                return Err(Failure::unchanged(invalid("prepared stage was replaced")));
            }
            check_target(&self.parent, &self.plan).map_err(Failure::unchanged)?;
            self.parent
                .rename_file(&self.plan.stage, &self.file, &self.plan.target, self.plan.expected.is_some())
                .map_err(|e| Failure::new(e, Visibility::Unknown))?;
            journal.observed(&self.plan, FileBoundary::NamespaceChanged);
            self.file.sync_all().map_err(|e| Failure::new(e, Visibility::Published))?;
            journal.observed(&self.plan, FileBoundary::PublishedSynced);
            self.parent.sync_chain().map_err(|e| Failure::new(e, Visibility::Published))?;
            journal.observed(&self.plan, FileBoundary::PublishedNamespaceSynced);
            journal
                .record(&self.plan, FileCheckpoint::Installed { identity: self.identity })
                .map_err(|e| Failure::new(e, Visibility::Published))?;
            Ok(PublishedFile { identity: self.identity, operation: self.plan.operation })
        })
    }
}

/// Recover only a persisted Prepared checkpoint. Incomplete stages use resume.
/// A missing or externally changed file is not an asset deletion or success.
/// The caller must keep the path unavailable until this returns successfully.
pub fn recover_prepared<G>(
    parent: Directory,
    plan: FilePlan,
    candidate: FileIdentity,
    journal: &mut impl FileJournal,
    check_content: impl FnOnce(&mut File, &FilePlan) -> io::Result<()>,
    verify: impl FnOnce() -> io::Result<G>,
) -> Result<PublishedFile, Failure> {
    crate::roles::in_place("recover materialized file", || {
        let _guard = verify().map_err(Failure::unchanged)?;
        plan.validate(&parent).map_err(Failure::unchanged)?;
        let installed = parent.file_identity(&plan.target).map_err(Failure::unchanged)? == Some(candidate);
        let name = if installed { &plan.target } else { &plan.stage };
        let mut file = parent.open_prepared(name).map_err(Failure::unchanged)?;
        if identity(&file).map_err(Failure::unchanged)? != candidate
            || file.metadata().map_err(Failure::unchanged)?.len() != plan.bytes
            || !plan.mode.matches(&file).map_err(Failure::unchanged)?
        {
            return Err(Failure::unchanged(invalid("prepared file identity, length or mode changed")));
        }
        check_content(&mut file, &plan).map_err(Failure::unchanged)?;
        if !installed {
            check_target(&parent, &plan).map_err(Failure::unchanged)?;
            parent
                .rename_file(&plan.stage, &file, &plan.target, plan.expected.is_some())
                .map_err(|e| Failure::new(e, Visibility::Unknown))?;
        }
        // Prepared was committed only after complete bytes/mode were flushed.
        // Unix additionally confirms the open file; Windows reopens read-only
        // prepared files with write-through metadata and DELETE, not WRITE_DATA.
        parent.confirm_prepared(&file).map_err(|e| Failure::new(e, Visibility::Published))?;
        parent.sync_chain().map_err(|e| Failure::new(e, Visibility::Published))?;
        journal
            .record(&plan, FileCheckpoint::Installed { identity: candidate })
            .map_err(|e| Failure::new(e, Visibility::Published))?;
        Ok(PublishedFile { identity: candidate, operation: plan.operation })
    })
}
fn check_target(parent: &Directory, plan: &FilePlan) -> io::Result<()> {
    if parent.file_identity(&plan.target)? != plan.expected {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "working file changed since intent"));
    }
    Ok(())
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}
