//! Atomic generation switches with Runtime-owned durable recovery intent.
//!
//! A generation checker must verify the accepted content root and completion
//! of every file's durable publication. A directory fsync alone is not that
//! evidence. Caller-held leases preserve both generations until installed is
//! committed and old handles have drained; this module never deletes them.

#[cfg(windows)]
mod windows;
use crate::{
    materialize_file::{Failure, Visibility},
    rooted_path::{Directory, FileIdentity, Name},
};
use fastctx_types::sync::identity::{Digest, OperationId};
use std::{io, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    /// Incoming staging name. Unix exchange puts the retired generation here;
    /// use ReadyDirectory::path for the installed path. Windows keeps this name
    /// as the physical generation directory behind the junction.
    pub name: Name,
    pub identity: FileIdentity,
    pub content: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryPlan {
    pub operation: OperationId,
    pub parent: FileIdentity,
    pub stable: Name,
    pub wanted: Generation,
    pub previous: Option<Generation>,
    /// Windows's registered junction identity. None for first installation or a
    /// recorded SlotRepair awaiting its replacement SlotCreated checkpoint.
    pub slot: Option<FileIdentity>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryCheckpoint {
    Intent,
    /// Persist the missing-slot repair before allocating a replacement. During
    /// replay, use slot=None until SlotCreated registers the replacement ID.
    SlotRepair,
    SlotCreated {
        identity: FileIdentity,
    },
    Switched,
    Installed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryBoundary {
    GenerationChecked,
    GenerationSynced,
    SlotCreated,
    NamespaceChanged,
    JunctionFlushed,
    NamespaceSynced,
}
/// Returning success attests to a FULL foundation Runtime commit, not a queued
/// write or a platform sidecar record.
/// Intent reserves this binding's slot and preserves both generation roots.
/// Replays compare operation/parent/stable/generations without regressing an
/// installed checkpoint. `slot` is reconstructed from SlotRepair/SlotCreated;
/// it is observed Runtime state, not part of that immutable intent identity.
pub trait DirectoryJournal {
    fn record(&mut self, plan: &DirectoryPlan, checkpoint: DirectoryCheckpoint) -> io::Result<()>;
    /// Volatile tracing only; never a substitute for a durable checkpoint.
    fn observed(&mut self, _plan: &DirectoryPlan, _boundary: DirectoryBoundary) {}
}

/// Only a successfully reconciled publication returns this token. Services must
/// not expose the path at startup until recover has produced one.
#[derive(Debug)]
pub struct ReadyDirectory {
    path: PathBuf,
    generation: Generation,
    retired: Option<Name>,
    slot: FileIdentity,
}
impl ReadyDirectory {
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    /// Model skill roots retain the slash required by `find "<skill>/"`.
    pub fn context_path(&self) -> String {
        format!("{}/", self.path.to_string_lossy().trim_end_matches(['/', '\\']))
    }
    pub fn generation(&self) -> &Generation {
        &self.generation
    }
    pub fn retired_name(&self) -> Option<&Name> {
        self.retired.as_ref()
    }
    pub fn slot_identity(&self) -> FileIdentity {
        self.slot
    }
}

/// The checker streams the complete manifest and refuses missing/mismatched
/// files or links. The final guard serializes wanted, ACL/exposure and cleanup
/// through the switch and installed commit; it must not hold a database writer.
pub fn publish<G>(
    parent: &Directory,
    plan: &DirectoryPlan,
    journal: &mut impl DirectoryJournal,
    check_generation: impl Fn(&Directory, &Generation) -> io::Result<()>,
    verify: impl FnOnce() -> io::Result<G>,
) -> Result<ReadyDirectory, Failure> {
    switch(parent, plan, journal, check_generation, verify, false)
}
/// Reconciles actual directory identity/junction target even if installed and
/// wanted were equal in the database. Replaying never exchanges generations back.
/// Missing or corrupt content stays blocked; no path token is returned.
pub fn recover<G>(
    parent: &Directory,
    plan: &DirectoryPlan,
    journal: &mut impl DirectoryJournal,
    check_generation: impl Fn(&Directory, &Generation) -> io::Result<()>,
    verify: impl FnOnce() -> io::Result<G>,
) -> Result<ReadyDirectory, Failure> {
    switch(parent, plan, journal, check_generation, verify, true)
}
fn switch<G>(
    parent: &Directory,
    plan: &DirectoryPlan,
    journal: &mut impl DirectoryJournal,
    check: impl Fn(&Directory, &Generation) -> io::Result<()>,
    verify: impl FnOnce() -> io::Result<G>,
    recovering: bool,
) -> Result<ReadyDirectory, Failure> {
    crate::roles::in_place("switch materialized generation", || {
        validate(parent, plan).map_err(Failure::unchanged)?;
        let wanted = locate_wanted(parent, plan).map_err(Failure::unchanged)?;
        check(&wanted, &plan.wanted).map_err(Failure::unchanged)?;
        journal.observed(plan, DirectoryBoundary::GenerationChecked);
        wanted.sync_chain().map_err(Failure::unchanged)?;
        journal.observed(plan, DirectoryBoundary::GenerationSynced);
        journal.record(plan, DirectoryCheckpoint::Intent).map_err(Failure::unchanged)?;
        let _guard = verify().map_err(Failure::unchanged)?;
        parent.verify_attached().map_err(Failure::unchanged)?;
        #[cfg(windows)]
        let slot = windows::switch(parent, plan, journal, recovering)?;
        #[cfg(unix)]
        let slot = {
            let _ = recovering;
            switch_unix(parent, plan, journal)?;
            plan.wanted.identity
        };
        journal.record(plan, DirectoryCheckpoint::Switched).map_err(|e| Failure::new(e, Visibility::Published))?;
        parent.sync_chain().map_err(|e| Failure::new(e, Visibility::Published))?;
        let actual = locate_wanted(parent, plan).map_err(|e| Failure::new(e, Visibility::Published))?;
        actual.sync_chain().map_err(|e| Failure::new(e, Visibility::Published))?;
        journal.observed(plan, DirectoryBoundary::NamespaceSynced);
        if actual.identity() != plan.wanted.identity {
            return Err(Failure::new(invalid("installed generation changed"), Visibility::Published));
        }
        journal.record(plan, DirectoryCheckpoint::Installed).map_err(|e| Failure::new(e, Visibility::Published))?;
        #[cfg(unix)]
        let retired = plan.previous.as_ref().map(|_| plan.wanted.name.clone());
        #[cfg(windows)]
        let retired = plan.previous.as_ref().map(|g| g.name.clone());
        Ok(ReadyDirectory {
            path: parent.display_path().join(plan.stable.as_os_str()),
            generation: plan.wanted.clone(),
            retired,
            slot,
        })
    })
}
fn validate(parent: &Directory, plan: &DirectoryPlan) -> io::Result<()> {
    if parent.identity() != plan.parent
        || plan.stable.could_alias(&plan.wanted.name)
        || plan.wanted.identity.volume != plan.parent.volume
        || plan.previous.as_ref().is_some_and(|g| {
            g.identity == plan.wanted.identity || (cfg!(windows) && g.name.could_alias(&plan.wanted.name))
        })
    {
        return Err(invalid("directory intent does not match its managed parent/generations"));
    }
    parent.verify_attached()?;
    parent.verify_private()
}
fn locate_wanted(parent: &Directory, plan: &DirectoryPlan) -> io::Result<Directory> {
    #[cfg(unix)]
    {
        if let Some(directory) = optional_directory(parent, &plan.stable)?
            && directory.identity() == plan.wanted.identity
        {
            return Ok(directory);
        }
    }
    let directory = parent.open_directory(&plan.wanted.name)?;
    if directory.identity() != plan.wanted.identity {
        return Err(invalid("wanted generation was replaced"));
    }
    Ok(directory)
}
#[cfg(unix)]
fn optional_directory(parent: &Directory, name: &Name) -> io::Result<Option<Directory>> {
    match parent.open_directory(name) {
        Ok(d) => Ok(Some(d)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
#[cfg(unix)]
fn switch_unix(parent: &Directory, plan: &DirectoryPlan, journal: &mut impl DirectoryJournal) -> Result<(), Failure> {
    let actual = optional_directory(parent, &plan.stable).map_err(Failure::unchanged)?;
    if actual.as_ref().is_some_and(|v| v.identity() == plan.wanted.identity) {
        return Ok(());
    }
    match (actual, &plan.previous) {
        (Some(current), Some(previous)) if current.identity() == previous.identity => {
            parent.exchange(&plan.stable, &plan.wanted.name).map_err(|e| Failure::new(e, Visibility::Unknown))?
        }
        (None, _) => parent
            .install_directory(&plan.wanted.name, &plan.stable)
            .map_err(|e| Failure::new(e, Visibility::Unknown))?,
        _ => {
            return Err(Failure::unchanged(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "stable directory differs from the recorded generation",
            )));
        }
    }
    journal.observed(plan, DirectoryBoundary::NamespaceChanged);
    let installed = parent.open_directory(&plan.stable).map_err(|e| Failure::new(e, Visibility::Published))?;
    if installed.identity() != plan.wanted.identity {
        return Err(Failure::new(invalid("directory switch produced an unexpected identity"), Visibility::Published));
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod health;
