//! Keeps this machine's copy of each town's ledger what its signed entries
//! say (`world-journals.md`, 只增不改，由世界守着). Residents and the node
//! run as the same user, so a resident can change the files; the node
//! watches them, and puts back what was changed or removed and moves out
//! what no entry wrote. Each time is a fact the World records, with what
//! was there, under the town's `tampering/`: that it happened is never
//! wiped away.

use std::collections::{BTreeMap, HashSet};
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

/// Checks every town now, then watches the towns and checks a town again
/// whenever its files change.
pub fn start(node: &Arc<Node>) -> io::Result<()> {
    let towns = node.world.path().join("towns");
    ensure_private_dir(&towns)?;
    let (changed, mut changes) = mpsc::unbounded_channel::<Option<TownName>>();
    let root = crate::file_events::named(&towns)?;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
        Ok(event) if !crate::file_events::changes(&event) => {}
        Ok(event) if !event.need_rescan() => {
            for path in event.paths {
                // Only the ledger's own files: the node's own records of
                // what it found are beside it.
                let Ok(rest) = path.strip_prefix(&root) else { continue };
                let mut names = rest.components().map(|name| name.as_os_str().to_string_lossy().into_owned());
                let Some(town) = names.next() else {
                    let _ = changed.send(None);
                    continue;
                };
                let part = names.next();
                if part.as_deref().is_none_or(|part| matches!(part, "ledger" | "journals" | "specs"))
                    && let Ok(town) = TownName::try_from(town)
                {
                    let _ = changed.send(Some(town));
                }
            }
        }
        // Something was missed: every town is checked again.
        _ => {
            let _ = changed.send(None);
        }
    })
    .map_err(io::Error::other)?;
    notify::Watcher::watch(&mut watcher, &towns, notify::RecursiveMode::Recursive).map_err(io::Error::other)?;
    let node = node.clone();
    tokio::spawn(async move {
        let _watcher = watcher;
        check_all(&node).await;
        while let Some(first) = changes.recv().await {
            let mut all = first.is_none();
            let mut towns: HashSet<TownName> = first.into_iter().collect();
            while let Ok(more) = changes.try_recv() {
                match more {
                    Some(town) => {
                        towns.insert(town);
                    }
                    None => all = true,
                }
            }
            if all {
                check_all(&node).await;
            } else {
                for town in towns {
                    check(&node, &town).await;
                }
            }
        }
    });
    Ok(())
}

async fn check_all(node: &Arc<Node>) {
    match node.world.towns() {
        Ok(towns) => {
            for town in towns {
                check(node, &town).await;
            }
        }
        Err(error) => node.log.line(format_args!("listing the towns to check their ledgers: {error}")),
    }
}

/// Sets one town's ledger right; then what its residents' Claude sessions
/// hold of its record follows it.
async fn check(node: &Arc<Node>, town: &TownName) {
    set_town_right(node, town).await;
    let (handing, name) = (node.clone(), town.clone());
    let _ = tokio::task::spawn_blocking(move || super::instructions::refresh(&handing, &name)).await;
}

/// Sets one town's ledger right, while no entry is being taken.
async fn set_town_right(node: &Arc<Node>, town: &TownName) {
    let _syncing = node.syncing.lock().await;
    let (world, name) = (node.world.clone(), town.clone());
    match tokio::task::spawn_blocking(move || {
        let mut found = set_right(&world, &name)?;
        found.extend(crate::memory::projection::restore(&world, &name)?.into_iter().map(|path| Tampered {
            at: Timestamp::now(),
            path,
            what: "memory copy restored from the ledger",
            found: None,
        }));
        io::Result::Ok(found)
    })
    .await
    {
        Ok(Ok(found)) => {
            for tampered in found {
                node.log.line(format_args!("{town}'s ledger: {} {}", tampered.path, tampered.what));
            }
        }
        Ok(Err(error)) => node.log.line(format_args!("checking {town}'s ledger: {error}")),
        Err(_) => {}
    }
}

/// Compares the town's ledger files with what its entries say, and sets
/// right what differs. Returns what it found.
fn set_right(world: &WorldDir, town: &TownName) -> io::Result<Vec<Tampered>> {
    let directory = world.town(town);
    let mut expected: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (_, entry) in crate::ledger::entries_after(world, town, 0)? {
        for file in entry.files {
            expected.insert(file.path, file.bytes.0);
        }
    }
    let ledger = directory.ledger();
    let mut present = Vec::new();
    walk(&ledger, &ledger, &mut present)?;
    let at = Timestamp::now();
    let record = directory.path().join("tampering").join(at.strftime("%Y%m%dT%H%M%S%.fZ").to_string());
    let mut found = Vec::new();
    for (path, file) in &present {
        match expected.get(path) {
            Some(bytes) if std::fs::read(file)? == *bytes => {}
            Some(bytes) => {
                let kept = keep_found(&record, path, file)?;
                rewrite(file, bytes)?;
                found.push(Tampered {
                    at,
                    path: path.clone(),
                    what: "was changed and was put back",
                    found: Some(kept),
                });
            }
            None => {
                let kept = keep_found(&record, path, file)?;
                writable(file)?;
                std::fs::remove_file(file)?;
                found.push(Tampered {
                    at,
                    path: path.clone(),
                    what: "is no entry's and was moved out",
                    found: Some(kept),
                });
            }
        }
    }
    let had: HashSet<&String> = present.iter().map(|(path, _)| path).collect();
    for (path, bytes) in &expected {
        if !had.contains(path) {
            let file = path.split('/').fold(ledger.clone(), |file, name| file.join(name));
            if let Some(parent) = file.parent() {
                ensure_private_dir(parent)?;
            }
            rewrite(&file, bytes)?;
            found.push(Tampered { at, path: path.clone(), what: "was removed and was put back", found: None });
        }
    }
    if !found.is_empty() {
        ensure_private_dir(&record)?;
        let mut bytes = serde_json::to_vec_pretty(&found).map_err(io::Error::other)?;
        bytes.push(b'\n');
        publish_new(&record.join("found.json"), &bytes, Permissions::Private)?;
    }
    Ok(found)
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
        let kind = std::fs::symlink_metadata(&path)?;
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

/// Keeps what was found at `path` beside the record of finding it.
fn keep_found(record: &Path, path: &str, file: &Path) -> io::Result<String> {
    let kept = format!("found/{path}");
    let target = kept.split('/').fold(record.to_path_buf(), |target, name| target.join(name));
    if let Some(parent) = target.parent() {
        ensure_private_dir(parent)?;
    }
    std::fs::copy(file, &target)?;
    Ok(kept)
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
