use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::apply::Report;
use crate::transaction::{FileChange, Snapshot, Transaction};

use super::{Error, files};

#[derive(Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Contents {
    version: u32,
    actions: Vec<Action>,
    observations: Vec<Observation>,
    #[serde(default)]
    archives: Vec<ArchiveAction>,
    #[serde(default)]
    directories: Vec<DirectoryAction>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ArchiveAction {
    object: super::archive::Object,
    destination: PathBuf,
    day: String,
    category: String,
    reason: String,
    outcome: String,
    #[serde(default)]
    copying: bool,
    #[serde(default)]
    slot: Option<String>,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct DirectoryAction {
    object: super::archive::Object,
    outcome: String,
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Action {
    path: PathBuf,
    before_sha256: String,
    after_sha256: Option<String>,
    reason: String,
    outcome: String,
}

#[derive(Clone, Hash, Eq, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Observation {
    location: String,
    reason: String,
}

/// What a cleanup record says it did, read without changing it.
pub(super) fn summary(path: &Path, root: &Path) -> Result<Option<super::Summary>, Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(super::io(error)),
    };
    let contents: Contents =
        serde_json::from_slice(&bytes).map_err(|_| Error::Io(format!("{}: invalid cleanup record", path.display())))?;
    let archived: Vec<&ArchiveAction> =
        contents.archives.iter().filter(|action| action.outcome == "completed").collect();
    let mut archives: Vec<PathBuf> =
        archived.iter().map(|action| root.join(format!("legacy-archive-{}", action.day))).collect();
    archives.dedup();
    let tidied = contents.actions.iter().filter(|action| action.outcome == "completed").count();
    let waiting = contents.archives.iter().filter(|action| unfinished(&action.outcome)).count()
        + contents.actions.iter().filter(|action| unfinished(&action.outcome)).count()
        + contents.directories.iter().filter(|action| unfinished(&action.outcome)).count();
    Ok(Some(super::Summary { record: path.to_path_buf(), archives, archived: archived.len(), tidied, waiting }))
}

type ObjectKey = (PathBuf, u64, u128);
type ActionKey = (PathBuf, String, Option<String>, String);

pub(super) struct Record {
    snapshot: Snapshot,
    contents: Contents,
    wrote: bool,
    archives: HashMap<ObjectKey, usize>,
    directories: HashMap<ObjectKey, usize>,
    actions: HashMap<ActionKey, usize>,
    descendants: HashMap<PathBuf, Vec<usize>>,
}

impl Record {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let snapshot = files::read(path)?;
        let contents: Contents = match snapshot.bytes() {
            None => Contents { version: 1, ..Contents::default() },
            Some(bytes) => serde_json::from_slice(bytes)
                .map_err(|_| Error::Io(format!("{}: invalid cleanup record; left untouched", path.display())))?,
        };
        if contents.version != 1 {
            return Err(Error::Io(format!("{}: unsupported cleanup record version", path.display())));
        }
        Ok(Self {
            snapshot,
            contents,
            wrote: false,
            archives: HashMap::new(),
            directories: HashMap::new(),
            actions: HashMap::new(),
            descendants: HashMap::new(),
        })
    }

    pub fn begin_batch(&mut self) {
        self.archives.clear();
        self.directories.clear();
        self.actions.clear();
        self.descendants.clear();
        for (index, action) in self.contents.archives.iter().enumerate() {
            if action.outcome == "pending" || action.outcome.starts_with("deferred:") {
                self.archives.entry(object_key(&action.object)).or_insert(index);
                for parent in action.object.path.ancestors().skip(1) {
                    self.descendants.entry(parent.to_path_buf()).or_default().push(index);
                }
            }
        }
        for (index, action) in self.contents.directories.iter().enumerate() {
            self.directories.entry(object_key(&action.object)).or_insert(index);
        }
        for (index, action) in self.contents.actions.iter().enumerate() {
            self.actions.entry(action_key(action)).or_insert(index);
        }
    }

    /// Resume older per-entry plans before retiring their containing directory.
    /// Discovery has authorized the directory; the record only supplies slots.
    pub fn inside(&self, object: &super::archive::Object) -> Vec<(super::archive::Object, String, String)> {
        self.descendants
            .get(&object.path)
            .into_iter()
            .flatten()
            .map(|index| &self.contents.archives[*index])
            .map(|action| (action.object.clone(), action.category.clone(), action.reason.clone()))
            .collect()
    }

    pub fn recover(
        &mut self,
        paths: &super::paths::Paths,
        places: &crate::apply::Places,
    ) -> Result<Vec<crate::Notice>, Error> {
        let mut recovered = Vec::new();
        for action in &mut self.contents.actions {
            if action.outcome != "pending" {
                continue;
            }
            if !paths.recovery_path(&action.path) {
                let name = action.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                let executable = places.fastctx.file_name().and_then(|name| name.to_str()).unwrap_or("");
                if action.after_sha256.is_none()
                    && super::processes::known_binary(&action.before_sha256)
                    && ["fastctx", "fastctx.exe", executable]
                        .iter()
                        .any(|base| super::leftovers::replacement_with_rf(name, base).is_some())
                {
                    files::check_path(
                        action.path.parent().ok_or_else(|| Error::Io("invalid replacement path".into()))?,
                    )
                    .map_err(super::io)?;
                    action.outcome = match std::fs::symlink_metadata(&action.path) {
                        Ok(_) => "interrupted replacement cleanup; source awaits independent rediscovery",
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            "replacement absent after interrupted cleanup"
                        }
                        Err(error) => return Err(super::io(error)),
                    }
                    .into();
                    continue;
                }
                return Err(Error::Io("cleanup record contains a path outside the artifact contract".into()));
            }
            // Recovery observes only; it never executes paths supplied by a record.
            // A subsequent discovery must independently authorize every mutation.
            let current = files::read(&action.path)?;
            let hash = current.bytes().map(files::hash);
            action.outcome = if hash == action.after_sha256 {
                "expected result observed after an interrupted attempt"
            } else if hash.as_ref() == Some(&action.before_sha256) {
                "interrupted before publication; eligible for a new attempt"
            } else {
                "changed during an interrupted attempt; preserved"
            }
            .into();
        }
        for (index, action) in self.contents.archives.iter_mut().enumerate() {
            if action.outcome != "pending" && !action.outcome.starts_with("deferred:") {
                continue;
            }
            if let Err(error) = validate_archive(action, &places.root, index) {
                action.outcome = format!("conflict: archive location changed; retained: {error}");
                recovered.push(crate::Notice {
                    location: action.destination.display().to_string(),
                    reason: action.outcome.clone(),
                });
                continue;
            }
            let source = match super::archive::Object::read(&action.object.path) {
                Ok(source) => source,
                Err(error) => {
                    recovered.push(crate::Notice {
                        location: action.object.path.display().to_string(),
                        reason: format!("interrupted archive source remains inaccessible: {error}"),
                    });
                    continue;
                }
            };
            let destination = match super::archive::Object::read(&action.destination) {
                Ok(destination) => destination,
                Err(error) => {
                    action.outcome = format!("conflict: archive location changed; retained: {error}");
                    recovered.push(crate::Notice {
                        location: action.destination.display().to_string(),
                        reason: action.outcome.clone(),
                    });
                    continue;
                }
            };
            if source.is_none()
                && let Some(destination) = destination
            {
                action.outcome = if destination.identity == action.object.identity {
                    "completed".into()
                } else {
                    match super::archive::completed_copy(&action.object, &action.destination) {
                        Ok(true) => {
                            action.copying = true;
                            "completed".into()
                        }
                        Ok(false) => {
                            "conflict: source absent and archive has no matching copy manifest; retained".into()
                        }
                        Err(error) => format!("conflict: source absent; archive retained: {error}"),
                    }
                };
                recovered.push(crate::Notice {
                    location: action.object.path.display().to_string(),
                    reason: format!("archive recovered at {}: {}", action.destination.display(), action.outcome),
                });
            } else if source.is_some() && source.as_ref() != Some(&action.object) {
                action.outcome = "source replaced since archive attempt; new object preserved".into();
            }
        }
        for action in &mut self.contents.directories {
            if action.outcome == "pending" {
                action.outcome = if super::archive::Object::read(&action.object.path).map_err(super::io)?.is_none() {
                    "directory absent after interrupted attempt"
                } else {
                    "interrupted before directory removal"
                }
                .into();
            }
        }
        Ok(recovered)
    }

    pub fn prepare_archive(
        &mut self,
        object: &super::archive::Object,
        category: &str,
        reason: &str,
        root: &Path,
    ) -> Result<(usize, PathBuf, bool), Error> {
        object.verify().map_err(super::io)?;
        if let Some(&index) = self.archives.get(&object_key(object)) {
            validate_archive(&self.contents.archives[index], root, index)?;
            self.contents.archives[index].outcome = "pending".into();
            let destination = self.contents.archives[index].destination.clone();
            let copying = self.contents.archives[index].copying;
            return Ok((index, destination, copying));
        }
        let index = self.contents.archives.len();
        let day = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date().to_string();
        let category_path =
            std::fs::canonicalize(root).map_err(super::io)?.join(format!("legacy-archive-{day}")).join(category);
        let slot_path = crate::disk::unique_path(&category_path, &format!("{index:06}-")).map_err(super::io)?;
        let slot = slot_path.file_name().expect("generated slot").to_string_lossy().into_owned();
        let destination =
            slot_path.join(object.path.file_name().ok_or_else(|| Error::Io("archive source has no name".into()))?);
        let action = ArchiveAction {
            object: object.clone(),
            destination: destination.clone(),
            day,
            category: category.into(),
            reason: reason.into(),
            outcome: "pending".into(),
            copying: false,
            slot: Some(slot),
        };
        validate_archive(&action, root, index)?;
        self.archives.insert(object_key(object), index);
        self.contents.archives.push(action);
        Ok((index, destination, false))
    }

    pub fn archive_copying(&mut self, index: usize) -> Result<(), Error> {
        self.contents.archives[index].copying = true;
        Ok(())
    }

    pub fn archive_complete(&mut self, index: usize) -> Result<(), Error> {
        self.contents.archives[index].outcome = "completed".into();
        Ok(())
    }

    pub fn archive_deferred(&mut self, index: usize, error: &std::io::Error) -> Result<(), Error> {
        self.contents.archives[index].outcome = if error.kind() == std::io::ErrorKind::InvalidData {
            format!("conflict: {error}; prior archive retained; next attempt gets a new slot")
        } else {
            format!("deferred: {error}")
        };
        Ok(())
    }

    pub fn archive_destinations(&self) -> Vec<PathBuf> {
        self.contents
            .archives
            .iter()
            .filter(|action| action.copying || unfinished(&action.outcome))
            .map(|action| action.destination.clone())
            .collect()
    }

    pub fn prepare_directory(&mut self, object: &super::archive::Object) -> Result<usize, Error> {
        let key = object_key(object);
        let index = *self.directories.entry(key).or_insert(self.contents.directories.len());
        if index == self.contents.directories.len() {
            self.contents.directories.push(DirectoryAction { object: object.clone(), outcome: "pending".into() });
        } else {
            self.contents.directories[index].outcome = "pending".into();
        }
        Ok(index)
    }

    pub fn directory_finished(&mut self, index: usize, outcome: String) -> Result<(), Error> {
        self.contents.directories[index].outcome = outcome;
        Ok(())
    }

    pub fn observe(&mut self, report: &Report) {
        let mut seen: HashSet<_> = self.contents.observations.iter().cloned().collect();
        for notice in &report.notices {
            let observation = Observation { location: notice.location.clone(), reason: notice.reason.clone() };
            if seen.insert(observation.clone()) {
                self.contents.observations.push(observation);
            }
        }
        for reason in &report.leftovers {
            let observation = Observation { location: "remaining after cleanup".into(), reason: reason.clone() };
            if seen.insert(observation.clone()) {
                self.contents.observations.push(observation);
            }
        }
    }

    pub fn prepare(&mut self, before: &Snapshot, after: Option<&[u8]>, reason: String) -> Result<usize, Error> {
        before.verify()?;
        let action = Action {
            path: before.path().into(),
            before_sha256: files::hash(before.bytes().expect("only existing artifacts are retired")),
            after_sha256: after.map(files::hash),
            reason,
            outcome: "pending".into(),
        };
        let key = action_key(&action);
        let index = match self.actions.get(&key).copied() {
            Some(index) => {
                self.contents.actions[index].outcome = "pending".into();
                index
            }
            None => {
                let index = self.contents.actions.len();
                self.actions.insert(key, index);
                self.contents.actions.push(action);
                index
            }
        };
        Ok(index)
    }

    pub fn complete(&mut self, index: usize) -> Result<(), Error> {
        self.contents.actions[index].outcome = "completed".into();
        Ok(())
    }

    pub fn deferred(&mut self, index: usize, reason: String) -> Result<(), Error> {
        self.contents.actions[index].outcome = format!("deferred: {reason}");
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), Error> {
        let mut bytes = serde_json::to_vec_pretty(&self.contents).map_err(|error| Error::Io(error.to_string()))?;
        bytes.push(b'\n');
        if self.snapshot.bytes() == Some(bytes.as_slice()) {
            return Ok(());
        }
        let receipt =
            Transaction::new([FileChange::private(self.snapshot.clone(), bytes)]).commit().map_err(|error| {
                Error::Files(format!(
                    "cleanup checkpoint could not be published; unfinished steps remain for the next startup: {error}"
                ))
            })?;
        if !receipt.cleanup.is_empty() {
            return Err(Error::Files(receipt.cleanup.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")));
        }
        self.snapshot = files::read(self.snapshot.path())?;
        self.wrote = true;
        Ok(())
    }

    pub fn wrote(&self) -> bool {
        self.wrote
    }
}

fn validate_archive(action: &ArchiveAction, root: &Path, index: usize) -> Result<(), Error> {
    let day =
        action.day.parse::<jiff::civil::Date>().map_err(|_| Error::Io("invalid archive date in record".into()))?;
    if day.to_string() != action.day
        || !["jobs", "update", "runtime", "early-names", "temporary", "executable-replacements", "settings", "pdf"]
            .contains(&action.category.as_str())
    {
        return Err(Error::Io("invalid archive category or date in record".into()));
    }
    let expected = std::fs::canonicalize(root)
        .map_err(super::io)?
        .join(format!("legacy-archive-{}", action.day))
        .join(&action.category)
        .join(match &action.slot {
            None => format!("{index:06}"),
            Some(slot)
                if slot.strip_prefix(&format!("{index:06}-")).is_some_and(|suffix| {
                    suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                }) =>
            {
                slot.clone()
            }
            Some(_) => return Err(Error::Io("invalid archive slot in record".into())),
        })
        .join(action.object.path.file_name().ok_or_else(|| Error::Io("invalid archive source name".into()))?);
    if action.destination != expected {
        return Err(Error::Io("archive destination differs from its assigned slot".into()));
    }
    files::check_path(expected.parent().expect("archive slot has a parent")).map_err(super::io)
}

fn object_key(object: &super::archive::Object) -> ObjectKey {
    (object.path.clone(), object.identity.volume, object.identity.file)
}

fn unfinished(outcome: &str) -> bool {
    outcome == "pending" || outcome.starts_with("deferred")
}

fn action_key(action: &Action) -> ActionKey {
    (action.path.clone(), action.before_sha256.clone(), action.after_sha256.clone(), action.reason.clone())
}
