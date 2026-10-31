//! Keeps this machine's copy of each town's ledger what its signed entries
//! say (`world-journals.md`, 只增不改，由世界守着). Residents and the node
//! run as the same user, so a resident can change the files; the node
//! watches them, and puts back what was changed or removed and moves out
//! what no entry wrote. Each time is a fact the World records, with what
//! was there, under the town's `tampering/`: that it happened is never
//! wiped away.
//!
//! The guard keeps in memory what each town's entries wrote, the digest of
//! each file and the entry that holds it, reading only the entries taken
//! since; a change is checked at the paths it touched. Writing an entry
//! costs its own files, however long the ledger (`world-foundation.md`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_platform::{Permissions, ensure_private_dir, publish_new};
use fastctx_types::TownName;
use fastctx_types::layout::WorldDir;
use jiff::Timestamp;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::Node;

/// One thing found and set right.
#[derive(Serialize)]
struct Tampered {
    at: Timestamp,
    /// Relative to the ledger.
    path: String,
    what: &'static str,
    /// Where what was found there is kept, relative to this record.
    found: Option<String>,
}

/// What changed under the towns, as the watcher saw it.
enum Changed {
    /// Something was missed: every town is checked whole.
    All,
    /// A town, whole.
    Town(TownName),
    /// One path in a town's ledger; whether what is under it is to be
    /// looked at too, as when a folder came, went or moved, not when only
    /// its own time changed with what changed in it, told of by itself.
    Path(TownName, String, bool),
}

/// Checks every town now, then watches the towns and checks again what
/// changes in a town's files.
pub fn start(node: &Arc<Node>) -> io::Result<()> {
    let towns = node.world.path().join("towns");
    ensure_private_dir(&towns)?;
    let (changed, mut changes) = mpsc::unbounded_channel::<Changed>();
    let root = crate::file_events::named(&towns)?;
    let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| match event {
        Ok(event) if !crate::file_events::changes(&event) => {}
        Ok(event) if !event.need_rescan() => {
            for path in event.paths {
                // Only the ledger's own files: the node's own records of
                // what it found are beside it.
                let Ok(rest) = path.strip_prefix(&root) else { continue };
                let names: Vec<String> =
                    rest.components().map(|name| name.as_os_str().to_string_lossy().into_owned()).collect();
                let Some(town) = names.first() else {
                    let _ = changed.send(Changed::All);
                    continue;
                };
                let Ok(town) = TownName::try_from(town.clone()) else { continue };
                let deep = !matches!(
                    event.kind,
                    notify::EventKind::Modify(
                        notify::event::ModifyKind::Data(_) | notify::event::ModifyKind::Metadata(_)
                    )
                );
                match names.get(1).map(String::as_str) {
                    Some("ledger") if names.len() > 2 => {
                        let _ = changed.send(Changed::Path(town, names[2..].join("/"), deep));
                    }
                    None | Some("ledger" | "journals" | "specs") => {
                        let _ = changed.send(Changed::Town(town));
                    }
                    Some(_) => {}
                }
            }
        }
        // Something was missed: every town is checked again.
        _ => {
            let _ = changed.send(Changed::All);
        }
    });
    // Only the ledger's own folders: whatever else a town holds spends no
    // watches.
    watcher
        .watch_within(&towns, |relative| {
            let mut parts = relative.components();
            parts.next();
            parts.next().is_none_or(|part| matches!(part.as_os_str().to_str(), Some("ledger" | "journals" | "specs")))
        })
        .map_err(io::Error::other)?;
    let node = node.clone();
    tokio::spawn(async move {
        let _watcher = watcher;
        let mut kept = HashMap::new();
        check_all(&node, &mut kept).await;
        while let Some(first) = changes.recv().await {
            let mut all = false;
            let mut whole = HashSet::new();
            let mut paths: HashMap<TownName, BTreeMap<String, bool>> = HashMap::new();
            for change in std::iter::once(first).chain(std::iter::from_fn(|| changes.try_recv().ok())) {
                match change {
                    Changed::All => all = true,
                    Changed::Town(town) => {
                        whole.insert(town);
                    }
                    Changed::Path(town, path, deep) => {
                        *paths.entry(town).or_default().entry(path).or_default() |= deep;
                    }
                }
            }
            if all {
                check_all(&node, &mut kept).await;
                continue;
            }
            for town in &whole {
                check(&node, &mut kept, town, None).await;
            }
            for (town, paths) in paths {
                if !whole.contains(&town) {
                    check(&node, &mut kept, &town, Some(paths)).await;
                }
            }
        }
    });
    Ok(())
}

async fn check_all(node: &Arc<Node>, kept: &mut HashMap<TownName, Expected>) {
    match node.world.towns() {
        Ok(towns) => {
            kept.retain(|town, _| towns.contains(town));
            for town in towns {
                check(node, kept, &town, None).await;
            }
        }
        Err(error) => node.log.line(format_args!("listing the towns to check their ledgers: {error}")),
    }
}

/// Sets right one town's ledger, whole or at `paths`; then, when its record
/// may have changed, what its residents' Claude sessions hold of the record
/// follows it.
async fn check(
    node: &Arc<Node>,
    kept: &mut HashMap<TownName, Expected>,
    town: &TownName,
    paths: Option<BTreeMap<String, bool>>,
) {
    if set_town_right(node, kept, town, paths).await {
        let (handing, name) = (node.clone(), town.clone());
        let _ = tokio::task::spawn_blocking(move || super::instructions::refresh(&handing, &name)).await;
    }
}

/// Sets one town's ledger right, while no entry is being taken. Whether the
/// town's record may have changed.
async fn set_town_right(
    node: &Arc<Node>,
    kept: &mut HashMap<TownName, Expected>,
    town: &TownName,
    paths: Option<BTreeMap<String, bool>>,
) -> bool {
    let _syncing = node.syncing.lock().await;
    let (world, name) = (node.world.clone(), town.clone());
    let mut expected = kept.remove(town).unwrap_or_default();
    let whole = paths.is_none();
    let checked = tokio::task::spawn_blocking(move || {
        let found = set_right(&world, &name, &mut expected, paths.as_ref());
        (expected, found)
    })
    .await;
    match checked {
        Ok((expected, Ok((found, record)))) => {
            kept.insert(town.clone(), expected);
            for tampered in found {
                node.log.line(format_args!("{town}'s ledger: {} {}", tampered.path, tampered.what));
            }
            whole || record
        }
        // What was read is let go, and read again next time.
        Ok((_, Err(error))) => {
            node.log.line(format_args!("checking {town}'s ledger: {error}"));
            true
        }
        Err(_) => true,
    }
}

/// What a town's entries wrote, as the guard last read them.
#[derive(Default)]
struct Expected {
    /// The ledger it was read from: one moved, or read anew, starts over.
    place: Option<crate::ledger::names::Place>,
    /// The last entry read.
    through: u64,
    /// Each file an entry wrote: its digest, and the entry that holds it.
    files: BTreeMap<String, ([u8; 32], u64)>,
}

impl Expected {
    /// Reads the entries taken since; whether one wrote the town's record.
    fn refresh(&mut self, world: &WorldDir, town: &TownName) -> io::Result<bool> {
        let place = crate::ledger::names::place(world, town)?;
        if place != self.place || crate::ledger::replica_seq(world, town)? < self.through {
            *self = Self { place, ..Self::default() };
        }
        let mut record = false;
        for (seq, entry) in crate::ledger::entries_after(world, town, self.through)? {
            for file in entry.files {
                record |= file.path.starts_with("project/");
                self.files.insert(file.path, (fastctx_net::sign::sha256(&file.bytes.0), seq));
            }
            self.through = seq;
        }
        Ok(record)
    }

    /// The paths an entry wrote at or under `path`.
    fn under<'a>(&'a self, path: &'a str) -> impl Iterator<Item = &'a String> + 'a {
        let below = format!("{path}/");
        self.files
            .get_key_value(path)
            .map(|(key, _)| key)
            .into_iter()
            .chain(self.files.range(below.clone()..).map(|(key, _)| key).take_while(move |key| key.starts_with(&below)))
    }

    /// The bytes the entry that wrote `path` gives it.
    fn bytes(&self, world: &WorldDir, town: &TownName, path: &str) -> io::Result<Vec<u8>> {
        let (_, seq) = self.files.get(path).ok_or_else(|| io::Error::other(format!("no entry wrote {path}")))?;
        let entry = crate::ledger::entry(world, town, *seq)?
            .ok_or_else(|| io::Error::other(format!("the entry that wrote {path} is gone")))?;
        entry
            .files
            .into_iter()
            .find(|file| file.path == path)
            .map(|file| file.bytes.0)
            .ok_or_else(|| io::Error::other(format!("the entry that wrote {path} holds no such file")))
    }
}

/// Compares the town's ledger files, whole or at `paths` (and under those
/// marked so), with what its entries say, and sets right what differs.
/// Returns what it found, and whether the town's record may have changed.
fn set_right(
    world: &WorldDir,
    town: &TownName,
    expected: &mut Expected,
    paths: Option<&BTreeMap<String, bool>>,
) -> io::Result<(Vec<Tampered>, bool)> {
    let directory = world.town(town);
    let mut record = expected.refresh(world, town)?;
    let ledger = directory.ledger();
    let mut present = Vec::new();
    let mut looked: BTreeSet<String> = BTreeSet::new();
    match paths {
        None => {
            walk(&ledger, &ledger, &mut present)?;
            looked.extend(expected.files.keys().cloned());
        }
        Some(paths) => {
            for (path, deep) in paths {
                record |= path.starts_with("project/") || path == "project";
                let file = path.split('/').fold(ledger.clone(), |file, name| file.join(name));
                match std::fs::symlink_metadata(&file) {
                    Ok(kind) if kind.is_dir() => {
                        if *deep {
                            walk(&ledger, &file, &mut present)?;
                            looked.extend(expected.under(path).cloned());
                        }
                    }
                    Ok(_) => {
                        present.push((path.clone(), file));
                        looked.insert(path.clone());
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        looked.extend(expected.under(path).cloned());
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    present.sort();
    present.dedup();
    let at = Timestamp::now();
    let record_dir = directory.path().join("tampering").join(at.strftime("%Y%m%dT%H%M%S%.fZ").to_string());
    let mut found = Vec::new();
    let mut had = HashSet::new();
    for (path, file) in &present {
        looked.insert(path.clone());
        // A file gone since it was listed is as one that was never there.
        let now = match std::fs::read(file) {
            Ok(now) => now,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(at_path(path, error)),
        };
        had.insert(path.clone());
        match expected.files.get(path) {
            Some((digest, _)) if fastctx_net::sign::sha256(&now) == *digest => {}
            Some(_) => {
                let bytes = expected.bytes(world, town, path)?;
                let kept = keep_found(&record_dir, path, &now)?;
                rewrite(file, &bytes).map_err(|error| at_path(path, error))?;
                found.push(Tampered {
                    at,
                    path: path.clone(),
                    what: "was changed and was put back",
                    found: Some(kept),
                });
            }
            None => {
                let kept = keep_found(&record_dir, path, &now)?;
                match writable(file).and_then(|()| std::fs::remove_file(file)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(at_path(path, error)),
                    _ => {}
                }
                found.push(Tampered {
                    at,
                    path: path.clone(),
                    what: "is no entry's and was moved out",
                    found: Some(kept),
                });
            }
        }
    }
    for path in &looked {
        if !had.contains(path) && expected.files.contains_key(path) {
            let file = path.split('/').fold(ledger.clone(), |file, name| file.join(name));
            if let Some(parent) = file.parent() {
                ensure_private_dir(parent)?;
            }
            rewrite(&file, &expected.bytes(world, town, path)?).map_err(|error| at_path(path, error))?;
            found.push(Tampered { at, path: path.clone(), what: "was removed and was put back", found: None });
        }
    }
    if !found.is_empty() {
        ensure_private_dir(&record_dir)?;
        let mut bytes = serde_json::to_vec_pretty(&found).map_err(io::Error::other)?;
        bytes.push(b'\n');
        publish_new(&record_dir.join("found.json"), &bytes, Permissions::Private)?;
        record |= found.iter().any(|tampered| tampered.path.starts_with("project/"));
    }
    Ok((found, record))
}

/// Every file under the ledger, by its path in the ledger.
fn walk(root: &Path, directory: &Path, files: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        // One gone since it was listed is passed over.
        let kind = match std::fs::symlink_metadata(&path) {
            Ok(kind) => kind,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if kind.is_dir() {
            walk(root, &path, files)?;
        } else {
            let relative = path.strip_prefix(root).map_err(io::Error::other)?;
            let names: Vec<String> =
                relative.components().map(|name| name.as_os_str().to_string_lossy().into_owned()).collect();
            files.push((names.join("/"), path));
        }
    }
    Ok(())
}

/// Keeps `found`, what was found at `path`, beside the record of finding it.
fn keep_found(record: &Path, path: &str, found: &[u8]) -> io::Result<String> {
    let kept = format!("found/{path}");
    let target = kept.split('/').fold(record.to_path_buf(), |target, name| target.join(name));
    if let Some(parent) = target.parent() {
        ensure_private_dir(parent)?;
    }
    std::fs::write(&target, found)?;
    Ok(kept)
}

fn at_path(path: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{path}: {error}"))
}

/// Writes a ledger file's bytes again, read-only as it was.
fn rewrite(file: &Path, bytes: &[u8]) -> io::Result<()> {
    if file.exists() {
        writable(file)?;
        std::fs::remove_file(file)?;
    }
    publish_new(file, bytes, Permissions::Private)?;
    let mut permissions = std::fs::metadata(file)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(file, permissions)
}

fn writable(file: &Path) -> io::Result<()> {
    let mut permissions = std::fs::metadata(file)?.permissions();
    if permissions.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(file, permissions)?;
    }
    Ok(())
}
