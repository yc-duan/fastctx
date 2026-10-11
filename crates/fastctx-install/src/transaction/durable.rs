//! Write-ahead publication and recovery. File objects are retained, not copied
//! and discarded: a process holding an old handle can continue writing them.

use super::*;
use sha2::{Digest, Sha256};

#[path = "collection.rs"]
mod collection;

pub use collection::collect_finished;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Version {
    identity: Identity,
    modified: std::time::SystemTime,
    digest: String,
    attributes: String,
    publication_attributes: String,
}

impl Version {
    fn of(snapshot: &Snapshot) -> Result<Option<Self>, FileError> {
        snapshot
            .state
            .as_ref()
            .map(|state| {
                let attributes = state
                    .attributes
                    .revision_bytes()
                    .map_err(|e| FileError::io(snapshot.path(), "encode attributes", e))?;
                let publication = state
                    .attributes
                    .publication_bytes()
                    .map_err(|e| FileError::io(snapshot.path(), "encode publication attributes", e))?;
                let mut hash = Sha256::new();
                hash.update((state.bytes.len() as u64).to_le_bytes());
                hash.update(&state.bytes);
                Ok(Self {
                    identity: state.identity,
                    modified: state.modified,
                    digest: format!("{:x}", hash.finalize()),
                    attributes: format!("{:x}", Sha256::digest(&attributes)),
                    publication_attributes: format!("{:x}", Sha256::digest(publication)),
                })
            })
            .transpose()
    }

    fn is_payload(&self, expected: &Self) -> bool {
        self.identity == expected.identity
            && self.modified == expected.modified
            && self.digest == expected.digest
            && self.publication_attributes == expected.publication_attributes
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Object {
    path: PathBuf,
    version: Version,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Restoration {
    source: Object,
    publication: PathBuf,
    displaced: PathBuf,
    expected: Option<Version>,
    conflict: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    target: PathBuf,
    before: Option<Version>,
    observed: Option<Object>,
    after: Option<Object>,
    publication: PathBuf,
    displaced: PathBuf,
    withdrawn: PathBuf,
    restorations: Vec<Restoration>,
    #[serde(default)]
    command_revisions: Vec<Object>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Directory {
    path: PathBuf,
    staging: PathBuf,
    identity: Identity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Preparing,
    Forward,
    Rollback,
    Committed,
    RolledBack,
}

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    version: u32,
    phase: Phase,
    manifest: PathBuf,
    parents: Vec<(PathBuf, Identity)>,
    directories: Vec<Directory>,
    entries: Vec<Entry>,
    #[serde(default)]
    guards: Vec<Guard>,
    created: Created,
    conflicts: Vec<String>,
    cleanup_on_commit: Vec<CreatedDirectory>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Guard {
    target: PathBuf,
    before: Option<Version>,
    anchor: (PathBuf, Identity),
}

/// Lists retained user-file archives, including cross-volume anchors. This
/// reads manifests only; it does not recover a transaction or open targets.
pub fn retained_archives(root: &Path) -> Result<Vec<PathBuf>, FileError> {
    let history = root.join("apply-history");
    let canonical_history = match fs::canonicalize(&history) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FileError::io(&history, "locate retained user versions", error)),
    };
    let entries = match fs::read_dir(&history) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FileError::io(&history, "list retained user versions", error)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let directory = entry.map_err(|error| FileError::io(&history, "list retained user versions", error))?.path();
        paths.push(directory.clone());
        let manifest = directory.join("manifest.json");
        if !manifest.is_file() {
            continue;
        }
        let journal = Journal::read(&manifest)?;
        let mut object = |path: &Path| {
            if !path.starts_with(&history)
                && !path.starts_with(&canonical_history)
                && let Some(parent) = path.parent()
            {
                paths.push(parent.to_path_buf());
            }
        };
        for entry in &journal.entries {
            for version in [&entry.observed, &entry.after].into_iter().flatten() {
                object(&version.path);
            }
            for revision in &entry.command_revisions {
                object(&revision.path);
            }
            for restoration in &entry.restorations {
                object(&restoration.source.path);
                object(&restoration.displaced);
            }
            object(&entry.displaced);
            object(&entry.withdrawn);
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Takes out of finished host publications the retained objects of files
/// inside `root` itself: earlier fastctx programs and the World's own
/// records, which an uninstall removes anyway and which are most of the
/// archive's size. Objects of the user's files stay, since a process
/// holding an old handle may have written to them; a publication left with
/// none of them goes whole. Returns the objects that could not be removed.
pub fn prune_owned(root: &Path) -> Result<Vec<(PathBuf, io::Error)>, FileError> {
    let history = root.join("apply-history");
    let entries = match fs::read_dir(&history) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(FileError::io(&history, "list retained versions", error)),
    };
    let roots =
        [root.to_path_buf(), fs::canonicalize(root).map_err(|error| FileError::io(root, "locate the World", error))?];
    let owned = |target: &Path| roots.iter().any(|root| target.starts_with(root));
    let mut kept = Vec::new();
    for entry in entries {
        let directory = entry.map_err(|error| FileError::io(&history, "list retained versions", error))?.path();
        let manifest = directory.join("manifest.json");
        if !manifest.is_file() {
            continue;
        }
        let journal = Journal::read(&manifest)?;
        if !matches!(journal.phase, Phase::Committed | Phase::RolledBack) {
            continue;
        }
        if journal.entries.iter().all(|entry| owned(&entry.target)) {
            if let Err(error) = fs::remove_dir_all(&directory) {
                kept.push((directory, error));
            }
            continue;
        }
        let homes = [directory.clone(), fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone())];
        for entry in journal.entries.iter().filter(|entry| owned(&entry.target)) {
            let mut objects = vec![&entry.publication, &entry.displaced, &entry.withdrawn];
            objects.extend([&entry.observed, &entry.after].into_iter().flatten().map(|object| &object.path));
            objects.extend(entry.command_revisions.iter().map(|object| &object.path));
            for restoration in &entry.restorations {
                objects.extend([&restoration.source.path, &restoration.publication, &restoration.displaced]);
            }
            for object in objects.into_iter().filter(|object| homes.iter().any(|home| object.starts_with(home))) {
                match fs::remove_file(object) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => kept.push((object.clone(), error)),
                }
            }
        }
    }
    // The empty archive container belongs to the World; any retained user
    // versions keep it nonempty and prevent its removal.
    if !crate::disk::is_link(
        &fs::symlink_metadata(&history).map_err(|error| FileError::io(&history, "inspect retained versions", error))?,
    ) {
        match fs::remove_dir(&history) {
            Ok(()) => {}
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty) => {}
            Err(error) => kept.push((history, error)),
        }
    }
    Ok(kept)
}

/// The next ownership record of an interrupted host publication. A preview
/// may use it to list homes without publishing or recovering anything.
pub fn pending_record(root: &Path) -> Result<Option<Snapshot>, FileError> {
    let active = Snapshot::read(root.join("applied-files-pending.json"))?;
    let Some(bytes) = active.bytes() else { return Ok(None) };
    let pointer: Active = serde_json::from_slice(bytes)
        .map_err(|error| FileError::io(active.path(), "read pending host record", io::Error::other(error)))?;
    let history = root.join("apply-history");
    if pointer.version != 1 || !pointer.manifest.starts_with(&history) {
        return Err(invalid(active.path(), "pending record belongs to another installation"));
    }
    let journal = Journal::read(&pointer.manifest)?;
    let target = root.join("applied.json");
    let object = journal.entries.iter().find(|entry| entry.target == target).and_then(|entry| entry.after.as_ref());
    let Some(object) = object else { return Ok(None) };
    if !object.path.starts_with(&history) {
        return Err(invalid(&object.path, "pending ownership record is outside this installation's archive"));
    }
    Ok(Some(Snapshot::read(&object.path)?))
}

#[derive(Serialize, Deserialize)]
struct Active {
    version: u32,
    manifest: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct SettingsCommand {
    target: PathBuf,
    before: Option<Version>,
    text: Option<String>,
}

/// Replays a prepared publication or finishes an interrupted rollback. The
/// caller must hold the installer lock before this call and before planning.
pub fn recover(active: impl AsRef<Path>) -> Result<Receipt, FileError> {
    let pointer = Snapshot::read(active)?;
    let active = pointer.path();
    let Some(bytes) = pointer.bytes() else { return Ok(Receipt::default()) };
    let pointer: Active = serde_json::from_slice(bytes)
        .map_err(|e| FileError::io(active, "read recovery pointer", io::Error::other(e)))?;
    let history = active.parent().expect("journal has a parent").join("apply-history");
    if pointer.version != 1 || !pointer.manifest.starts_with(&history) {
        return Err(invalid(active, "unsupported recovery pointer or different installation root"));
    }
    let mut journal = Journal::read(&pointer.manifest)?;
    journal.observe_commands()?;
    match journal.phase {
        Phase::Preparing => {
            journal.phase = Phase::RolledBack;
            journal.save()?;
        }
        Phase::Forward => {
            if let Err(cause) = journal.forward() {
                let failures = journal.rollback();
                let mut retained = true;
                if failures.is_empty() {
                    journal.retire_publications()?;
                    clear_active(active)?;
                    if journal.conflicts.is_empty() {
                        collection::discard(&journal)?;
                        retained = false;
                    }
                }
                return Err(journal.failure(cause, &failures, retained));
            }
        }
        Phase::Rollback => {
            let failures = journal.rollback();
            if let Some(cause) = failures.first() {
                return Err(invalid(active, &format!("{cause}; recovery evidence: {}", journal.manifest.display())));
            }
        }
        Phase::Committed | Phase::RolledBack => {}
    }
    journal.retire_publications()?;
    let receipt = journal.receipt();
    clear_active(active)?;
    if !journal.conflicts.is_empty() {
        return Err(invalid(
            active,
            &format!("{}\npreserved versions: {}", journal.conflicts.join("\n"), journal.manifest.display()),
        ));
    }
    collection::discard(&journal)?;
    Ok(receipt)
}

/// Records rollback CLI intent independently of the replaceable manifest. The
/// manifest itself may be the file that prevented a completed commit checkpoint.
pub(crate) fn prepare_settings_command(active: &Path, before: &Snapshot) -> Result<(), FileError> {
    let pointer = Snapshot::read(active)?;
    let Some(bytes) = pointer.bytes() else { return Ok(()) };
    let pointer: Active = serde_json::from_slice(bytes)
        .map_err(|error| FileError::io(active, "read recovery pointer", io::Error::other(error)))?;
    let journal = Journal::read(&pointer.manifest)?;
    if !matches!(journal.phase, Phase::Rollback | Phase::Forward) {
        return Ok(());
    }
    let Some(index) = journal.entries.iter().position(|entry| entry.target == before.path()) else { return Ok(()) };
    let entry = &journal.entries[index];
    let previous = Version::of(before)?;
    let mut known = payload_matches(previous.as_ref(), entry.after.as_ref().map(|object| &object.version));
    for command in journal.commands()?.iter().filter(|command| command.target == entry.target) {
        known |= command.matches(before)?;
    }
    if !known {
        return Ok(());
    }
    let text = before
        .bytes()
        .map(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
        .transpose()
        .map_err(|error| invalid(before.path(), &error.to_string()))?;
    let command = SettingsCommand { target: entry.target.clone(), before: previous, text };
    let path = disk::unique_path(journal.manifest.parent().unwrap(), "settings-command-")
        .map_err(|error| FileError::io(before.path(), "name CLI intent", error))?;
    let bytes = serde_json::to_vec(&command).map_err(|error| invalid(&path, &error.to_string()))?;
    fastctx_platform::publish_new(&path, &bytes, fastctx_platform::Permissions::Private)
        .map_err(|error| FileError::io(&path, "retain rollback CLI intent", error))?;
    sync_file(&path)?;
    disk::sync_parent(&path).map_err(|error| FileError::io(&path, "sync CLI intent", error))
}

impl SettingsCommand {
    fn matches(&self, current: &Snapshot) -> Result<bool, FileError> {
        let unmanaged = crate::claude::registration::same_unmanaged(
            self.text.as_ref().map(|text| text.as_bytes()),
            current.bytes(),
        );
        // Invalid JSON is an unrecognized external version. Recovery must
        // still preserve it and report the conflict, rather than stop parsing
        // before reaching the byte-preserving rollback.
        // Claude's temp-and-rename writer creates fresh metadata, including
        // creation time and ACL inheritance. Attribute equality cannot identify
        // its result; the retained object still preserves those attributes.
        Ok(matches!(unmanaged, Ok(true)))
    }
}

pub(super) fn commit(
    mut changes: Vec<FileChange>,
    active: &Path,
    record: Option<(Snapshot, RenderRecord<'_>)>,
    cleanup: Vec<CreatedDirectory>,
) -> Result<Receipt, Failure> {
    let mut journal = None;
    let result = (|| {
        if Snapshot::read(active)?.exists() {
            return Err(invalid(active, "an earlier transaction must be recovered before planning"));
        }
        let render = record.map(|(snapshot, render)| {
            changes.push(FileChange::private(snapshot, Vec::new()));
            render
        });
        validate_unique(&changes)?;
        for change in &changes {
            change.preflight()?;
        }
        journal = Some(Journal::start(active, cleanup)?);
        let journal = journal.as_mut().expect("started");
        for change in &changes {
            if change.after.is_some() {
                journal.parents(&change.before)?;
                if !change.before.exists() {
                    journal.created.files.push(change.before.path.clone());
                }
            }
        }
        if let Some(render) = render {
            let record = changes.last_mut().expect("record appended");
            record.after = Some(
                render(&journal.created).map_err(|e| FileError::io(record.before.path(), "render record", e))?.into(),
            );
        }
        for change in &changes {
            change.preflight()?;
            if change.changes() {
                journal.prepare(change)?;
            } else {
                journal.guards.push(Guard {
                    target: change.before.path.clone(),
                    before: Version::of(&change.before)?,
                    anchor: change.before.anchor.clone(),
                });
            }
        }
        if !journal.guards.is_empty() {
            journal.version = 2;
        }
        journal.phase = Phase::Forward;
        journal.save()?;
        journal.forward()
    })();
    match result {
        Ok(()) => {
            let journal = journal.expect("successful preparation");
            let mut receipt = journal.receipt();
            if let Err(error) = journal
                .retire_publications()
                .and_then(|()| clear_active(active))
                .and_then(|()| collection::discard(&journal))
            {
                receipt.cleanup.push(error);
                receipt.preserved.push(journal.manifest.clone());
            }
            Ok(receipt)
        }
        Err(cause) => {
            let Some(mut journal) = journal else {
                return Err(Failure {
                    cause: Box::new(cause),
                    rollback: vec![],
                    cleanup: vec![],
                    created: Created::default(),
                });
            };
            let rollback = journal.rollback();
            let mut retained = true;
            let mut cleanup = vec![];
            if rollback.is_empty() {
                if let Err(error) = journal.retire_publications() {
                    cleanup.push(error);
                } else {
                    cleanup.extend(journal.cleanup_directories());
                    if let Err(error) = clear_active(active) {
                        cleanup.push(error);
                    } else if journal.conflicts.is_empty() {
                        match collection::discard(&journal) {
                            Ok(()) => retained = false,
                            Err(error) => cleanup.push(error),
                        }
                    }
                }
            }
            let cause = journal.failure(cause, &rollback, retained);
            Err(Failure { cause: Box::new(cause), rollback, cleanup, created: journal.created })
        }
    }
}

impl Journal {
    fn commands(&self) -> Result<Vec<SettingsCommand>, FileError> {
        let directory = self.manifest.parent().unwrap();
        let mut commands = vec![];
        for item in fs::read_dir(directory).map_err(|error| FileError::io(directory, "read command intents", error))? {
            let path = item.map_err(|error| FileError::io(directory, "read command intent name", error))?.path();
            if path.file_name().unwrap().to_string_lossy().starts_with("settings-command-") {
                let file = Snapshot::read(&path)?;
                let command = serde_json::from_slice(file.bytes().ok_or_else(|| FileError::changed(&path))?)
                    .map_err(|error| invalid(&path, &format!("invalid CLI intent: {error}")))?;
                commands.push(command);
            }
        }
        Ok(commands)
    }

    fn observe_commands(&mut self) -> Result<(), FileError> {
        if !matches!(self.phase, Phase::Forward | Phase::Rollback) {
            return Ok(());
        }
        let commands = self.commands()?;
        if commands.is_empty() {
            return Ok(());
        }
        // Commands in an active file transaction are compensating mutations.
        // Their intent wins over an older Forward phase even if that phase's
        // replacement was locked or the process died before recording results.
        self.phase = Phase::Rollback;
        for entry in &mut self.entries {
            if !commands.iter().any(|command| command.target == entry.target) {
                continue;
            }
            // Attribution must not prevent the ordinary rollback from
            // restoring other entries and reporting this entry's I/O error.
            let Ok(current) = Snapshot::read(&entry.target) else { continue };
            let mut known = false;
            for command in commands.iter().filter(|command| command.target == entry.target) {
                known |= command.matches(&current)?;
            }
            if !known {
                continue;
            }
            let Some(version) = Version::of(&current)? else { continue };
            if entry.command_revisions.iter().any(|object| object.version == version) {
                continue;
            }
            let path = disk::unique_path(entry.displaced.parent().unwrap(), "cli-settings-")
                .map_err(|error| FileError::io(&entry.target, "name CLI revision", error))?;
            fs::hard_link(&entry.target, &path).map_err(|error| FileError::io(&path, "retain CLI revision", error))?;
            if version_at(&path)?.as_ref() != Some(&version) {
                return Err(FileError::changed(&entry.target));
            }
            disk::sync_parent(&path).map_err(|error| FileError::io(&path, "sync CLI revision", error))?;
            entry.command_revisions.push(Object { path, version });
        }
        self.save()
    }

    fn start(active: &Path, cleanup_on_commit: Vec<CreatedDirectory>) -> Result<Self, FileError> {
        let resolved = Snapshot::read(active)?;
        let active = resolved.path();
        let history = active.parent().expect("journal has parent").join("apply-history");
        disk::ensure_private_dir(&history).map_err(|e| FileError::io(&history, "create history", e))?;
        let directory =
            disk::unique_path(&history, "transaction-").map_err(|e| FileError::io(&history, "name transaction", e))?;
        disk::ensure_private_dir(&directory).map_err(|e| FileError::io(&directory, "create transaction", e))?;
        let journal = Self {
            version: 1,
            phase: Phase::Preparing,
            manifest: directory.join("manifest.json"),
            parents: vec![],
            directories: vec![],
            entries: vec![],
            guards: vec![],
            created: Created::default(),
            conflicts: vec![],
            cleanup_on_commit,
        };
        journal.save()?;
        let pointer = serde_json::to_vec(&Active { version: 1, manifest: journal.manifest.clone() })
            .map_err(|e| FileError::io(active, "encode recovery pointer", io::Error::other(e)))?;
        fastctx_platform::publish_new(active, &pointer, fastctx_platform::Permissions::Private)
            .map_err(|e| FileError::io(active, "publish recovery pointer", e))?;
        sync_file(active)?;
        disk::sync_parent(active).map_err(|e| FileError::io(active, "sync recovery pointer", e))?;
        Ok(journal)
    }

    fn read(path: &Path) -> Result<Self, FileError> {
        let snapshot = Snapshot::read(path)?;
        let bytes = snapshot.bytes().ok_or_else(|| invalid(path, "recovery manifest is missing"))?;
        let journal: Self = serde_json::from_slice(bytes)
            .map_err(|e| FileError::io(path, "read recovery manifest", io::Error::other(e)))?;
        if !matches!(journal.version, 1 | 2) || journal.manifest != snapshot.path() && journal.manifest != path {
            return Err(invalid(path, "unsupported or misplaced recovery manifest"));
        }
        Ok(journal)
    }

    fn save(&self) -> Result<(), FileError> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| FileError::io(&self.manifest, "encode recovery manifest", io::Error::other(e)))?;
        disk::publish_private(&self.manifest, &bytes)
            .map_err(|e| FileError::io(&self.manifest, "publish recovery manifest", e))
    }

    fn parents(&mut self, snapshot: &Snapshot) -> Result<(), FileError> {
        snapshot.verify()?;
        let parent = snapshot.path.parent().expect("absolute path");
        let mut path = snapshot.anchor.0.clone();
        self.parents.push(snapshot.anchor.clone());
        for part in parent.strip_prefix(&path).expect("resolved beneath anchor").components().collect::<Vec<_>>() {
            self.verify_parents(&path)?;
            path.push(part);
            if !path.exists() {
                let staging = disk::unique_path(path.parent().expect("directory parent"), ".fastctx-directory-")
                    .map_err(|e| FileError::io(&path, "name directory", e))?;
                fs::create_dir(&staging).map_err(|e| FileError::io(&staging, "stage directory", e))?;
                disk::sync_parent(&staging).map_err(|e| FileError::io(&staging, "sync staged directory", e))?;
                let identity = directory_identity(&staging)?;
                self.directories.push(Directory { path: path.clone(), staging: staging.clone(), identity });
                self.created.directories.push(CreatedDirectory { path: path.clone(), identity: Some(identity) });
                self.save()?;
                disk::move_preserving(&staging, &path).map_err(|e| FileError::io(&path, "publish directory", e))?;
                disk::sync_parent(&path).map_err(|e| FileError::io(&path, "sync directory", e))?;
                if directory_identity(&path)? != identity {
                    return Err(FileError::changed(&path));
                }
            }
            self.parents.push((path.clone(), directory_identity(&path)?));
        }
        self.save()
    }

    fn verify_parents(&self, target: &Path) -> Result<(), FileError> {
        for (path, identity) in &self.parents {
            if target.starts_with(path) && directory_identity(path)? != *identity {
                return Err(FileError::changed(path));
            }
        }
        Ok(())
    }

    fn archive(&self, snapshot: &Snapshot) -> Result<PathBuf, FileError> {
        let directory = self.manifest.parent().expect("manifest directory");
        if directory_identity(directory)?.volume == snapshot.anchor.1.volume {
            return Ok(directory.to_path_buf());
        }
        // The exchange/backup must stay on the target's filesystem. The
        // external archive is private and is named in the durable manifest.
        let directory =
            snapshot.anchor.0.join(format!(".fastctx-{}", directory.file_name().unwrap().to_string_lossy()));
        disk::ensure_private_dir(&directory).map_err(|e| FileError::io(&directory, "create same-volume archive", e))?;
        Ok(directory)
    }

    fn prepare(&mut self, change: &FileChange) -> Result<(), FileError> {
        let directory = self.archive(&change.before)?;
        let index = self.entries.len();
        let before = Version::of(&change.before)?;
        let observed = if let Some(version) = &before {
            let path = directory.join(format!("{index}-observed"));
            fs::hard_link(change.before.path(), &path)
                .map_err(|e| FileError::io(&path, "retain observed version", e))?;
            let actual = version_at(&path)?;
            if actual.as_ref() != Some(version) {
                return Err(self.conflict(change.before.path(), &[&path]));
            }
            disk::sync_parent(&path).map_err(|e| FileError::io(&path, "sync retained name", e))?;
            Some(Object { path, version: version.clone() })
        } else {
            None
        };
        let after = if change.after.is_some() {
            // Stage beside the destination so inherited ACLs come from its
            // directory, not the private archive. The permanent hard link
            // retains that same object without changing its ACL.
            let mut prepared = prepare_with_attributes(change, &change.before, false)?.expect("changed write");
            let path = directory.join(format!("{index}-new"));
            fs::hard_link(&prepared.file.path, &path).map_err(|e| FileError::io(&path, "retain prepared file", e))?;
            let version = Version::of(&prepared.installed)?.expect("staged file");
            // The journal owns this object now. It remains linked even after
            // publication, so subsequent writes through the installed file survive.
            prepared.file.discard().map_err(|e| FileError::io(&prepared.file.path, "remove preparation name", e))?;
            drop(prepared);
            sync_file(&path)?;
            disk::sync_parent(&path).map_err(|e| FileError::io(&path, "sync prepared name", e))?;
            Some(Object { path, version })
        } else {
            None
        };
        let publication = disk::unique_path(change.before.path.parent().unwrap(), ".fastctx-publish-")
            .map_err(|e| FileError::io(change.before.path(), "name publication", e))?;
        let displaced = directory.join(format!("{index}-displaced"));
        self.entries.push(Entry {
            target: change.before.path.clone(),
            before,
            observed,
            after,
            withdrawn: directory.join(format!("{index}-withdrawn")),
            publication,
            displaced,
            restorations: vec![],
            command_revisions: vec![],
        });
        self.save()
    }

    fn forward(&mut self) -> Result<(), FileError> {
        self.verify_guards()?;
        for entry in &self.entries {
            self.verify_guards()?;
            self.verify_parents(&entry.target)?;
            let current = version_at(&entry.target)?;
            let displaced = displaced_at(entry)?;
            if let Some(after) = &entry.after {
                if current.as_ref().is_some_and(|v| v.identity == after.version.identity) {
                    if !payload_matches(current.as_ref(), Some(&after.version)) {
                        return Err(self.conflict(&entry.target, &[&after.path]));
                    }
                } else if current == entry.before && displaced.is_none() {
                    verify_object(after)?;
                    publish_object(after, &entry.target, &entry.publication, &entry.displaced, current.is_some())?;
                } else if current.is_none() && displaced == entry.before && displaced.is_some() {
                    // ReplaceFile can move the old object aside before failing.
                    link_new(after, &entry.target)?;
                } else {
                    return Err(self.conflict(&entry.target, &[&entry.displaced, &after.path]));
                }
            } else if current == entry.before && displaced.is_none() {
                disk::move_preserving(&entry.target, &entry.displaced)
                    .map_err(|e| FileError::io(&entry.target, "retain deleted file", e))?;
            } else if current.is_some() || displaced.is_none() {
                return Err(self.conflict(&entry.target, &[&entry.displaced, &entry.publication]));
            }
            archive_displaced(&entry.displaced, &entry.publication, entry.after.as_ref())?;
            sync_names(&entry.target, &entry.displaced)?;
            self.verify_parents(&entry.target)?;
            let current = version_at(&entry.target)?;
            if !payload_matches(current.as_ref(), entry.after.as_ref().map(|o| &o.version))
                || displaced_at(entry)? != entry.before
            {
                return Err(self.conflict(&entry.target, &[&entry.displaced, &entry.publication]));
            }
        }
        self.verify_guards()?;
        self.phase = Phase::Committed;
        self.save()
    }

    fn verify_guards(&self) -> Result<(), FileError> {
        // Adopting existing values may only write a receipt. These unchanged
        // inputs must remain the same through publication and crash recovery.
        for guard in &self.guards {
            if directory_identity(&guard.anchor.0)? != guard.anchor.1 {
                return Err(FileError::changed(&guard.anchor.0));
            }
            self.verify_parents(&guard.target)?;
            if version_at(&guard.target)? != guard.before {
                return Err(FileError::changed(&guard.target));
            }
        }
        Ok(())
    }

    fn rollback(&mut self) -> Vec<FileError> {
        if self.phase == Phase::Preparing {
            self.phase = Phase::RolledBack;
            return self.save().err().into_iter().collect();
        }
        self.phase = Phase::Rollback;
        if let Err(error) = self.save() {
            return vec![error];
        }
        let mut errors = vec![];
        for index in (0..self.entries.len()).rev() {
            if let Err(error) = self.restore(index) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            self.phase = Phase::RolledBack;
            if let Err(error) = self.save() {
                errors.push(error);
            }
        }
        errors
    }

    fn restore(&mut self, index: usize) -> Result<(), FileError> {
        let entry = self.entries[index].clone();
        self.verify_parents(&entry.target)?;
        archive_displaced(&entry.displaced, &entry.publication, entry.after.as_ref())?;
        let current = version_at(&entry.target)?;
        let displaced_object = retained_object(&entry.displaced, &entry.publication, entry.after.as_ref())?;
        let displaced = displaced_object.as_ref().map(|object| object.version.clone());
        for restoration in &entry.restorations {
            archive_displaced(&restoration.displaced, &restoration.publication, Some(&restoration.source))?;
            if let Some(removed) = restoration_displaced(restoration)?
                && (Some(removed) != restoration.expected || restoration.conflict)
            {
                self.note_conflict(&entry.target, &[&restoration.displaced, &restoration.source.path])?;
            }
        }
        if let Some(restored) = entry.restorations.last()
            && current.as_ref().is_some_and(|v| v.identity == restored.source.version.identity)
        {
            // The completed restoration remains linked in the archive too.
            // Preserve subsequent edits instead of replaying snapshot bytes.
            return Ok(());
        }
        if displaced.is_none() {
            if entry.before.is_none() {
                if current.as_ref().is_some_and(|current| {
                    entry.after.as_ref().is_none_or(|after| current.identity != after.version.identity)
                }) {
                    return Err(self.conflict(&entry.target, &[&entry.target, &entry.withdrawn]));
                }
                if current.is_some() && version_at(&entry.withdrawn)?.is_none() {
                    disk::move_preserving(&entry.target, &entry.withdrawn)
                        .map_err(|e| FileError::io(&entry.target, "withdraw created file", e))?;
                    sync_names(&entry.target, &entry.withdrawn)?;
                }
                if let Some(removed) = version_at(&entry.withdrawn)?
                    && Some(removed) != entry.after.as_ref().map(|o| o.version.clone())
                {
                    self.note_conflict(&entry.target, &[&entry.withdrawn])?;
                }
                if version_at(&entry.target)?.is_some() {
                    return Err(self.conflict(&entry.target, &[&entry.withdrawn]));
                }
                return Ok(());
            }
            // No displaced object means this replacement did not take effect.
            return Ok(());
        }
        let retained = displaced.expect("checked above");
        if current.as_ref().is_some_and(|v| v.identity == retained.identity) {
            return Ok(());
        }
        let conflict = current.is_some()
            && !payload_matches(current.as_ref(), entry.after.as_ref().map(|o| &o.version))
            && !entry.command_revisions.iter().any(|object| current.as_ref() == Some(&object.version));
        // A restoration uses a fresh copy. ReplaceFile reopens its replacement
        // without sharing; the retained original may still have an outside
        // writer. Its original object and all subsequent writes remain archived.
        let original = Snapshot::read(&displaced_object.expect("retained object").path)?;
        let bytes = original.bytes().ok_or_else(|| FileError::changed(original.path()))?;
        let reverse =
            FileChange::write(Snapshot { path: entry.target.clone(), state: None, ..original.clone() }, bytes);
        let mut prepared = prepare_restore(&reverse, &original)?.expect("restoration copy");
        let retained_copy = disk::unique_path(entry.displaced.parent().unwrap(), "restored-object-")
            .map_err(|e| FileError::io(&entry.displaced, "name restoration object", e))?;
        fs::hard_link(&prepared.file.path, &retained_copy)
            .map_err(|e| FileError::io(&retained_copy, "retain restoration object", e))?;
        let source =
            Object { path: retained_copy, version: Version::of(&prepared.installed)?.expect("restoration file") };
        prepared
            .file
            .discard()
            .map_err(|e| FileError::io(&prepared.file.path, "remove restoration staging name", e))?;
        drop(prepared);
        sync_names(&source.path, &source.path)?;
        let publication = disk::unique_path(entry.target.parent().unwrap(), ".fastctx-restore-")
            .map_err(|e| FileError::io(&entry.displaced, "name restoration", e))?;
        let displaced = disk::unique_path(entry.displaced.parent().unwrap(), "rollback-displaced-")
            .map_err(|e| FileError::io(&entry.displaced, "name displaced restoration", e))?;
        let restoration = Restoration { source, publication, displaced, expected: current, conflict };
        self.entries[index].restorations.push(restoration.clone());
        self.save()?;
        let target = &entry.target;
        publish_object(
            &restoration.source,
            target,
            &restoration.publication,
            &restoration.displaced,
            restoration.expected.is_some(),
        )?;
        sync_names(target, &restoration.displaced)?;
        self.verify_parents(target)?;
        let removed = restoration_displaced(&restoration)?;
        if restoration.conflict
            || removed != restoration.expected
            || !payload_matches(version_at(target)?.as_ref(), Some(&restoration.source.version))
        {
            self.note_conflict(target, &[&restoration.displaced, &restoration.source.path])?;
        }
        Ok(())
    }

    fn cleanup_directories(&self) -> Vec<FileError> {
        let mut directories = self.created.directories.clone();
        directories.extend(
            self.directories.iter().map(|d| CreatedDirectory { path: d.staging.clone(), identity: Some(d.identity) }),
        );
        super::cleanup_directories(&directories)
    }

    fn retire_publications(&self) -> Result<(), FileError> {
        for entry in &self.entries {
            for path in std::iter::once(&entry.publication).chain(entry.restorations.iter().map(|r| &r.publication)) {
                if version_at(path)?.is_none() {
                    continue;
                }
                let destination = entry.displaced.parent().unwrap().join(path.file_name().unwrap());
                disk::move_preserving(path, &destination)
                    .map_err(|error| FileError::io(path, "retain unused publication name", error))?;
                sync_names(path, &destination)?;
            }
        }
        Ok(())
    }

    fn receipt(&self) -> Receipt {
        Receipt {
            changed: self.entries.iter().map(|entry| entry.target.clone()).collect(),
            created: self.created.clone(),
            cleanup: if self.phase == Phase::RolledBack {
                self.cleanup_directories()
            } else {
                super::cleanup_directories(&self.cleanup_on_commit)
            },
            preserved: if self.conflicts.is_empty() { vec![] } else { vec![self.manifest.clone()] },
        }
    }

    fn conflict(&self, target: &Path, versions: &[&Path]) -> FileError {
        invalid(
            target,
            &format!(
                "concurrent change; could not safely restore; your versions are preserved at {}; recovery manifest: {}",
                versions.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
                self.manifest.display()
            ),
        )
    }

    fn note_conflict(&mut self, target: &Path, versions: &[&Path]) -> Result<(), FileError> {
        self.conflicts.push(self.conflict(target, versions).to_string());
        self.save()
    }

    fn failure(&self, cause: FileError, rollback: &[FileError], retained: bool) -> FileError {
        invalid(
            &cause.path,
            &format!(
                "{cause}{}{}",
                rollback.iter().map(|e| format!("\nrollback: {e}")).collect::<String>(),
                if retained {
                    format!("\npreserved file versions and recovery state: {}", self.manifest.display())
                } else {
                    String::new()
                }
            ),
        )
    }
}

fn version_at(path: &Path) -> Result<Option<Version>, FileError> {
    Version::of(&Snapshot::read(path)?)
}

fn displaced_at(entry: &Entry) -> Result<Option<Version>, FileError> {
    Ok(retained_object(&entry.displaced, &entry.publication, entry.after.as_ref())?.map(|object| object.version))
}

fn restoration_displaced(restoration: &Restoration) -> Result<Option<Version>, FileError> {
    Ok(retained_object(&restoration.displaced, &restoration.publication, Some(&restoration.source))?
        .map(|object| object.version))
}

fn retained_object(displaced: &Path, publication: &Path, new: Option<&Object>) -> Result<Option<Object>, FileError> {
    if let Some(version) = version_at(displaced)? {
        return Ok(Some(Object { path: displaced.to_owned(), version }));
    }
    if !cfg!(windows)
        && let Some(version) = version_at(publication)?
        && new.is_none_or(|new| version.identity != new.version.identity)
    {
        return Ok(Some(Object { path: publication.to_owned(), version }));
    }
    Ok(None)
}

fn archive_displaced(displaced: &Path, publication: &Path, new: Option<&Object>) -> Result<(), FileError> {
    if let Some(object) = retained_object(displaced, publication, new)?
        && object.path != displaced
    {
        disk::move_preserving(&object.path, displaced)
            .map_err(|error| FileError::io(publication, "archive exchanged object", error))?;
        sync_names(publication, displaced)?;
    }
    Ok(())
}

fn verify_object(object: &Object) -> Result<(), FileError> {
    if !payload_matches(version_at(&object.path)?.as_ref(), Some(&object.version)) {
        return Err(FileError::changed(&object.path));
    }
    Ok(())
}

fn link_new(object: &Object, destination: &Path) -> Result<(), FileError> {
    verify_object(object)?;
    fs::hard_link(&object.path, destination)
        .map_err(|e| FileError::io(destination, "publish without replacement", e))?;
    sync_names(destination, &object.path)
}

fn publish_object(
    object: &Object,
    target: &Path,
    publication: &Path,
    displaced: &Path,
    replace: bool,
) -> Result<(), FileError> {
    if !replace {
        return link_new(object, target);
    }
    verify_object(object)?;
    if let Some(existing) = version_at(publication)? {
        if !existing.is_payload(&object.version) {
            return Err(FileError::changed(publication));
        }
    } else {
        fs::hard_link(&object.path, publication)
            .map_err(|e| FileError::io(publication, "stage retained publication", e))?;
        sync_names(publication, &object.path)?;
    }
    if cfg!(windows) && Snapshot::read(displaced)?.exists() {
        return Err(invalid(displaced, "refusing to replace an already retained version"));
    }
    disk::replace_preserving(publication, target, displaced)
        .map_err(|e| FileError::io(target, "publish retaining displaced version", e))
}

fn sync_file(path: &Path) -> Result<(), FileError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| FileError::io(path, "open for flush", e))?;
    file.sync_all().map_err(|e| FileError::io(path, "flush retained file", e))
}

fn sync_names(first: &Path, second: &Path) -> Result<(), FileError> {
    for path in [first, second] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if path == first && metadata.is_file() && !disk::is_link(&metadata) => sync_file(path)?,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(FileError::io(path, "inspect published name", error)),
        }
        disk::sync_parent(path).map_err(|e| FileError::io(path, "sync published name", e))?;
    }
    Ok(())
}

fn clear_active(path: &Path) -> Result<(), FileError> {
    fs::remove_file(path).map_err(|e| FileError::io(path, "clear recovery pointer", e))?;
    disk::sync_parent(path).map_err(|e| FileError::io(path, "sync recovery completion", e))
}

fn invalid(path: &Path, message: &str) -> FileError {
    FileError::io(path, "transaction recovery", io::Error::other(message.to_owned()))
}

fn payload_matches(actual: Option<&Version>, expected: Option<&Version>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(actual), Some(expected)) => actual.is_payload(expected),
        _ => false,
    }
}
