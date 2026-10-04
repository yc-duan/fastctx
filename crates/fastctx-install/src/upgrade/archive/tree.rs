//! A cross-volume archive is published in full before any source is retired.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::disk::{self, Attributes, Identity};
use crate::transaction::{FileChange, Transaction};

use super::{Object, conflict, files, metadata};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Entry {
    // OsString serialization preserves non-UTF-8 names on Unix.
    names: Vec<OsString>,
    identity: Identity,
    data: Data,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
enum Data {
    File { attributes: Attributes, modified: SystemTime, sha256: String },
    Directory { attributes: Attributes, modified: SystemTime },
    Special(metadata::Special),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    source: Object,
    entries: Vec<Entry>,
}

impl Entry {
    fn path(&self, root: &Path) -> PathBuf {
        let mut result = root.to_path_buf();
        result.extend(&self.names);
        result
    }

    fn read(path: &Path, names: Vec<OsString>, locks: &[File]) -> io::Result<Self> {
        let object = Object::read(path)?.ok_or_else(|| conflict("archive entry disappeared"))?;
        let metadata = fs::symlink_metadata(&object.path)?;
        let data = if disk::is_link(&metadata) || !(metadata.is_file() || metadata.is_dir()) {
            Data::Special(metadata::Special::read(&object.path)?)
        } else {
            let mut file = if metadata.is_dir() {
                disk::open_directory(&object.path)?
            } else {
                super::source_file(&object, locks)?
            };
            if disk::identity(&file)? != object.identity {
                return Err(conflict("source replaced while opening"));
            }
            let attributes = Attributes::read(&file, &object.path)?;
            let modified = file.metadata()?.modified()?;
            if metadata.is_file() {
                Data::File { attributes, modified, sha256: super::hash_handle(&mut file)? }
            } else {
                Data::Directory { attributes, modified }
            }
        };
        object.verify()?;
        Ok(Self { names, identity: object.identity, data })
    }

    fn verify(&self, root: &Path, original: bool, retiring: bool, locks: &[File]) -> io::Result<()> {
        let path = self.path(root);
        let current = Self::read(&path, self.names.clone(), locks)?;
        let matches = match (&self.data, &current.data) {
            // Unlinking the copied children changes the source directory time.
            (Data::Directory { attributes: a, .. }, Data::Directory { attributes: b, .. }) if retiring => a == b,
            (a, b) => a == b,
        };
        if !matches || (original && current.identity != self.identity) {
            return Err(conflict(format!("{} changed; neither version was overwritten", path.display())));
        }
        Ok(())
    }
}

impl Manifest {
    fn collect(object: &Object, locks: &[File]) -> io::Result<Self> {
        let mount = metadata::mount(&object.path)?;
        let mut entries = Vec::new();
        let mut pending = vec![Vec::new()];
        while let Some(names) = pending.pop() {
            let mut path = object.path.clone();
            path.extend(&names);
            if metadata::mount(&path)? != mount {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("{} is a nested mount; original tree retained", path.display()),
                ));
            }
            let entry = Entry::read(&path, names.clone(), locks)?;
            if matches!(entry.data, Data::Directory { .. }) {
                for child in files::entries(&path)?.into_iter().rev() {
                    let mut names = names.clone();
                    names.push(child.file_name().expect("directory entry").into());
                    pending.push(names);
                }
            }
            entries.push(entry);
        }
        let manifest = Self { source: object.clone(), entries };
        manifest.verify(&object.path, true, false, locks)?;
        Ok(manifest)
    }

    fn read(path: &Path) -> io::Result<Option<Self>> {
        let snapshot = files::read(path).map_err(io::Error::other)?;
        let Some(bytes) = snapshot.bytes() else {
            return Ok(None);
        };
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| conflict(format!("archive manifest is invalid; retained unchanged: {error}")))?;
        let mut seen = std::collections::HashSet::new();
        for entry in &manifest.entries {
            for name in &entry.names {
                let components: Vec<_> = Path::new(name).components().collect();
                if components.len() != 1 || !matches!(components[0], Component::Normal(_)) {
                    return Err(conflict("invalid relative name in archive manifest"));
                }
            }
            if !seen.insert(entry.names.clone()) {
                return Err(conflict("duplicate name in archive manifest"));
            }
        }
        if manifest
            .entries
            .first()
            .is_none_or(|entry| !entry.names.is_empty() || entry.identity != manifest.source.identity)
        {
            return Err(conflict("archive manifest has no matching root"));
        }
        Ok(Some(manifest))
    }

    fn verify(&self, root: &Path, original: bool, retiring: bool, locks: &[File]) -> io::Result<()> {
        let mut links = HashMap::new();
        let mut child_counts = HashMap::<&[OsString], usize>::new();
        for entry in &self.entries {
            if !entry.names.is_empty() {
                *child_counts.entry(&entry.names[..entry.names.len() - 1]).or_default() += 1;
            }
        }
        for entry in &self.entries {
            entry.verify(root, original, retiring, locks)?;
            let path = entry.path(root);
            if matches!(entry.data, Data::Directory { .. }) {
                let actual = files::entries(&path)?;
                let expected = child_counts.get(entry.names.as_slice()).copied().unwrap_or(0);
                if actual.len() != expected {
                    return Err(conflict(format!("{} has new or missing entries", path.display())));
                }
            }
            if matches!(entry.data, Data::File { .. }) {
                let identity = Object::read(&path)?.ok_or_else(|| conflict("archive disappeared"))?.identity;
                if let Some(first) = links.insert((entry.identity.volume, entry.identity.file), identity)
                    && first != identity
                {
                    return Err(conflict("archive hardlink relationship differs from source"));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn transfer(object: &Object, destination: &Path, locks: &[File], previous: &[PathBuf]) -> io::Result<()> {
    let slot = destination.parent().expect("archive slot");
    let manifest_path = slot.join("copy-manifest.json");
    let manifest = match Manifest::read(&manifest_path)? {
        Some(manifest) => {
            if manifest.source != *object {
                return Err(conflict("archive manifest names another source"));
            }
            manifest
        }
        None => {
            if fs::symlink_metadata(destination).is_ok() {
                return Err(conflict("archive already exists without its copy manifest"));
            }
            let manifest = Manifest::collect(object, locks)?;
            let bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
            let before = files::read(&manifest_path).map_err(io::Error::other)?;
            let receipt = Transaction::new([FileChange::private(before, bytes)]).commit().map_err(io::Error::other)?;
            if !receipt.cleanup.is_empty() {
                return Err(io::Error::other(format!("manifest publication: {:?}", receipt.cleanup)));
            }
            manifest
        }
    };
    match fs::symlink_metadata(destination) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            manifest.verify(&object.path, true, false, locks)?;
            let stage = disk::unique_path(slot, ".incomplete-copy-")?;
            let references = references(&manifest, destination, previous)?;
            copy(&manifest, &stage, locks, &references)?;
            manifest.verify(&stage, false, false, &[])?;
            manifest.verify(&object.path, true, false, locks)?;
            super::rename_new(&stage, destination)?;
            disk::sync_parent(destination)?;
        }
        Err(error) => return Err(error),
    }
    manifest.verify(destination, false, false, &[])?;
    // Publication is the durable checkpoint. From here on, existing archive
    // objects are only read, never repaired or overwritten from the source.
    for entry in manifest.entries.iter().rev() {
        let source = entry.path(&object.path);
        if Object::read(&source)?.is_none() {
            continue;
        }
        entry.verify(&object.path, true, true, locks)?;
        entry.verify(destination, false, false, &[])?;
        match &entry.data {
            Data::Special(_) => metadata::delete_special(&source)?,
            Data::Directory { .. } => super::delete_copied(&disk::open_directory(&source)?, &source)?,
            Data::File { .. } => {
                let source_object = Object { path: source.clone(), identity: entry.identity };
                super::delete_copied(&super::source_file(&source_object, locks)?, &source)?;
            }
        }
        disk::sync_parent(&source)?;
    }
    Ok(())
}

type References = HashMap<(u64, u128), Vec<(Data, PathBuf)>>;

fn references(manifest: &Manifest, destination: &Path, previous: &[PathBuf]) -> io::Result<References> {
    let mut references = References::new();
    let current_paths: std::collections::HashSet<_> =
        manifest.entries.iter().map(|entry| entry.path(&manifest.source.path)).collect();
    for other in previous {
        if other == destination {
            continue;
        }
        let sidecar = other.parent().expect("recorded archive slot").join("copy-manifest.json");
        // A completed archive belongs to the user. Deleting or moving it does
        // not prevent future cleanup; only extant, verified copies are reused.
        let Ok(Some(prior)) = Manifest::read(&sidecar) else {
            continue;
        };
        for entry in &prior.entries {
            if matches!(entry.data, Data::File { .. }) && !current_paths.contains(&entry.path(&prior.source.path)) {
                references
                    .entry((entry.identity.volume, entry.identity.file))
                    .or_default()
                    .push((entry.data.clone(), entry.path(other)));
            }
        }
    }
    // Incomplete copies are read-only candidates. A new attempt never changes
    // one, so edits made after interruption survive alongside the final copy.
    for path in files::entries(destination.parent().expect("archive slot"))? {
        if !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".incomplete-copy-")) {
            continue;
        }
        for entry in &manifest.entries {
            if matches!(entry.data, Data::File { .. }) {
                references
                    .entry((entry.identity.volume, entry.identity.file))
                    .or_default()
                    .push((entry.data.clone(), entry.path(&path)));
            }
        }
    }
    Ok(references)
}

fn copy(manifest: &Manifest, stage: &Path, locks: &[File], references: &References) -> io::Result<()> {
    let mut links = HashMap::<(u64, u128), PathBuf>::new();
    for entry in &manifest.entries {
        let source = entry.path(&manifest.source.path);
        let destination = entry.path(stage);
        entry.verify(&manifest.source.path, true, false, locks)?;
        match &entry.data {
            Data::Directory { .. } => fs::create_dir(&destination)?,
            Data::Special(metadata) => metadata.create(&source, &destination)?,
            Data::File { attributes, modified, .. } => {
                let key = (entry.identity.volume, entry.identity.file);
                let reference = links.get(&key).cloned().or_else(|| {
                    references.get(&key).and_then(|candidates| {
                        candidates
                            .iter()
                            .find(|(data, path)| {
                                *data == entry.data
                                    && Entry::read(path, entry.names.clone(), &[])
                                        .is_ok_and(|value| value.data == entry.data)
                            })
                            .map(|(_, path)| path.clone())
                    })
                });
                if let Some(first) = reference {
                    fs::hard_link(first, &destination)?;
                } else {
                    let object = Object { path: source, identity: entry.identity };
                    let mut reader = super::source_file(&object, locks)?;
                    let mut target = disk::create(&destination, 0o600)?;
                    attributes.protect(&target)?;
                    io::copy(&mut reader, &mut target)?;
                    attributes.apply(&target, &destination)?;
                    disk::restore_modified(&target, *modified)?;
                    target.sync_all()?;
                }
                links.insert(key, destination.clone());
            }
        }
    }
    for entry in manifest.entries.iter().rev() {
        if let Data::Directory { attributes, modified } = &entry.data {
            let path = entry.path(stage);
            let target = super::directory_for_attributes(&path)?;
            attributes.protect(&target)?;
            attributes.apply_directory(&target, &path)?;
            disk::restore_modified(&target, *modified)?;
            super::sync_directory(&target)?;
        }
    }
    disk::sync_parent(stage)
}
