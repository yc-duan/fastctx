//! This machine's copies of the ledgers of the towns it spans
//! (`world-network.md`, 共享账本), kept in each town's directory
//! (`fastctx_types::layout::TownDir`).
//!
//! What counts is the signed entry; the files are what it says, written for
//! residents to read with the tools they have. An entry is taken by keeping
//! its signed form first and then writing its files, so a node that stopped
//! in between finds the entry and finishes its files when it starts again;
//! a file already there must hold the same bytes. Entries are taken in
//! order, each once, and the files are left read-only.
//!
//! A machine in a World with others takes entries from the hub, which
//! orders them. A World of one machine keeps its ledgers itself, numbers
//! entries as it writes them, and signs them with its own key, made the
//! first time it needs one. When it joins a World, the towns it kept alone
//! are carried into it: each entry written again, in order, signed for that
//! World.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::ledger::{self, Entry, File};
use fastctx_net::link::Link;
use fastctx_net::sign::{Bytes, SecretKey, Signed};
use fastctx_net::trust::WorldId;
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::{TownDir, WorldDir};
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;

use crate::Node;

pub(crate) mod query;
pub use query::query;

/// Writes an entry into `town`'s ledger: `files`, at paths relative to the
/// ledger, and for a version of the town's record the machines it spans
/// from then on. Returns once this machine has the accepted entry, including
/// when it wrote on behalf of a device into a town that does not span it.
pub async fn write(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
) -> Result<(), Failure> {
    for (path, _) in &files {
        ledger::check_path(path).map_err(Failure::new)?;
    }
    let files: Vec<File> = files.into_iter().map(|(path, bytes)| File { path, bytes: Bytes(bytes) }).collect();
    match node.link() {
        Some(link) => {
            let entry = Entry {
                world: link.membership().world.clone(),
                town: town.clone(),
                author: node.machine(),
                at: Timestamp::now(),
                machines,
                files,
            };
            link.ledger_write(&entry).await.map_err(Failure::new)?;
            query(node, Some(town)).await.map(|_| ()).map_err(Failure::new)
        }
        None => {
            let (world, here, town) = (node.world.clone(), node.machine(), town.clone());
            let _writing = node.syncing.lock().await;
            tokio::task::spawn_blocking(move || write_alone(&world, &here, &town, files, machines))
                .await
                .map_err(|error| Failure::new(format!("writing the ledger: {error}")))?
                .map_err(Failure::new)
        }
    }
}

/// A World of one machine: the entry is checked as the hub would check it,
/// numbered, and taken here.
fn write_alone(
    world: &WorldDir,
    here: &MachineName,
    town: &TownName,
    files: Vec<File>,
    machines: Option<Vec<MachineName>>,
) -> Result<(), String> {
    let directory = world.town(town);
    let entries = read_all(&directory).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
    if entries.is_empty() && machines.is_none() {
        return Err(format!("the town {town} has no ledger yet; its first entry says which machines it spans"));
    }
    if let Some(machines) = &machines
        && let Some(stranger) = machines.iter().find(|machine| *machine != here)
    {
        return Err(format!("{stranger} is not a machine of this World; it has only {here}"));
    }
    let known: Vec<&str> =
        entries.iter().flat_map(|(_, entry, _)| entry.files.iter().map(|file| file.path.as_str())).collect();
    for (index, file) in files.iter().enumerate() {
        if files[..index].iter().any(|other| ledger::conflicts(&other.path, &file.path)) {
            return Err(format!("the entry names {} twice", file.path));
        }
        if let Some(taken) = known.iter().find(|path| ledger::conflicts(path, &file.path)) {
            return Err(format!("{town} has {taken} already; nothing in a ledger is written twice"));
        }
    }
    let key = own_key(world).map_err(|error| format!("this machine's key: {error}"))?;
    let entry = Entry {
        world: alone_world(world).map_err(|error| format!("this machine's World: {error}"))?,
        town: town.clone(),
        author: here.clone(),
        at: Timestamp::now(),
        machines,
        files,
    };
    let signed = ledger::sign(&key, &entry);
    let seq = entries.last().map(|(seq, _, _)| *seq).unwrap_or(0) + 1;
    apply(world, seq, &entry, &signed).map_err(|error| format!("keeping the entry: {error}"))
}

/// Takes what the hub has that this machine does not, for `town` or for
/// every town it spans, after carrying in the towns kept while alone. One
/// sync runs at a time.
pub async fn sync(node: &Arc<Node>, town: Option<TownName>) -> Result<(), String> {
    let Some(link) = node.link() else { return Ok(()) };
    let _syncing = node.syncing.lock().await;
    let at_hub: BTreeMap<TownName, u64> = link.ledger_towns().await?.into_iter().collect();
    let mut problems = Vec::new();
    if town.is_none() {
        problems.extend(carry(node, &link, &at_hub).await);
    }
    // What was carried is at the hub now; ask again for what it holds.
    let at_hub: BTreeMap<TownName, u64> =
        if problems.is_empty() { link.ledger_towns().await?.into_iter().collect() } else { at_hub };
    for (name, latest) in at_hub {
        if town.as_ref().is_some_and(|town| *town != name) {
            continue;
        }
        if let Err(error) = take(node, &link, &name, latest, false).await {
            problems.push(error);
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

/// Takes `town`'s entries after the ones this machine has, up to `latest`.
pub(super) async fn take(
    node: &Arc<Node>,
    link: &Link,
    town: &TownName,
    latest: u64,
    query: bool,
) -> Result<(), String> {
    let directory = node.world.town(town);
    let mut after = cursor(&directory).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
    while after < latest {
        let (entries, more) =
            if query { link.ledger_query(town, after).await? } else { link.ledger_read(town, after).await? };
        if entries.is_empty() {
            return Err(format!("the hub promised {latest} entries of {town}, but stopped after {after}"));
        }
        for (seq, entry, signed) in entries {
            if seq != after + 1 {
                return Err(format!("the hub gave entry {seq} of {town} after {after}"));
            }
            let world = node.world.clone();
            tokio::task::spawn_blocking(move || apply(&world, seq, &entry, &signed))
                .await
                .map_err(|error| format!("taking entry {seq} of {town}: {error}"))?
                .map_err(|error| format!("taking entry {seq} of {town}: {error}"))?;
            after = seq;
        }
        if !more && after < latest {
            return Err(format!("the hub ended {town}'s ledger at {after}, before its promised {latest}"));
        }
    }
    Ok(())
}

/// Carries every town this machine kept outside the World it is in now:
/// its entries are written to the hub again, in order, by this machine and
/// for this World, and the copies signed for the old one are let go. A town
/// the hub has some entries of already, the same ones, is carried on from
/// there, so a carry cut short finishes the next time. Returns what could
/// not be carried.
async fn carry(node: &Arc<Node>, link: &Link, at_hub: &BTreeMap<TownName, u64>) -> Vec<String> {
    let world = link.membership().world.clone();
    let here = node.machine();
    let towns = match node.world.towns() {
        Ok(towns) => towns,
        Err(error) => return vec![format!("reading the towns: {error}")],
    };
    let mut problems = Vec::new();
    for town in towns {
        let directory = node.world.town(&town);
        let entries = match read_all(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                problems.push(format!("reading {town}'s ledger: {error}"));
                continue;
            }
        };
        let elsewhere = entries.iter().filter(|(_, entry, _)| entry.world != world).count();
        if elsewhere == 0 {
            continue;
        }
        if elsewhere < entries.len() {
            problems.push(format!(
                "{town} has entries of this World and of another; it is left as it is in {}",
                directory.path().display()
            ));
            continue;
        }
        if let Err(error) =
            carry_town(link, &world, &here, &town, &entries, at_hub.get(&town).copied().unwrap_or(0)).await
        {
            problems.push(error);
            continue;
        }
        // The World's copy is taken afresh: it may call this machine by
        // another name than the files here do.
        if let Err(error) = remove(&directory.signed()).and_then(|()| remove(&directory.ledger())) {
            problems.push(format!("letting go of {town}'s ledger of its old World: {error}"));
            continue;
        }
        node.log.line(format_args!("carried {town} into the World {world}: {} entries", entries.len()));
    }
    problems
}

async fn carry_town(
    link: &Link,
    world: &WorldId,
    here: &MachineName,
    town: &TownName,
    entries: &[(u64, Entry, Signed)],
    at_hub: u64,
) -> Result<(), String> {
    let taken = format!("a town named {town} is in this World already; this machine's own {town} was not carried");
    // Each entry as the World gets it: by this machine, under the name it
    // has now.
    let mut carried = Vec::new();
    for (_, entry, _) in entries {
        let old = &entry.author;
        let renamed = |name: &MachineName| if name == old { here.clone() } else { name.clone() };
        let mut files = Vec::new();
        for file in &entry.files {
            let bytes = if old == here {
                file.bytes.clone()
            } else {
                Bytes(crate::town::rename::file(&file.path, &file.bytes.0, old, here)?)
            };
            files.push(File { path: file.path.clone(), bytes });
        }
        carried.push(Entry {
            world: world.clone(),
            town: town.clone(),
            author: here.clone(),
            at: entry.at,
            machines: entry.machines.as_ref().map(|machines| machines.iter().map(renamed).collect()),
            files,
        });
    }
    // What the hub has must be the start of this town, as a carry cut short
    // left it.
    let mut after = 0;
    while after < at_hub {
        let (read, more) = link.ledger_read(town, after).await?;
        for (seq, entry, _) in read {
            let ours = carried.get((seq - 1) as usize).ok_or_else(|| taken.clone())?;
            if ours.files != entry.files {
                return Err(taken);
            }
            after = seq;
        }
        if !more {
            break;
        }
    }
    for entry in carried.iter().skip(after as usize) {
        link.ledger_write(entry).await.map_err(|error| {
            if error.contains("already") { format!("{taken} ({error})") } else { format!("carrying {town}: {error}") }
        })?;
    }
    Ok(())
}

/// Removes a directory of ledger files, read-only as they are.
fn remove(directory: &Path) -> io::Result<()> {
    fn writable(path: &Path) -> io::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                writable(&entry?.path())?;
            }
        } else if metadata.permissions().readonly() {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            std::fs::set_permissions(path, permissions)?;
        }
        Ok(())
    }
    match writable(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        other => other?,
    }
    std::fs::remove_dir_all(directory)
}

/// Takes entry `seq`: its signed form, then its files.
fn apply(world: &WorldDir, seq: u64, entry: &Entry, signed: &Signed) -> io::Result<()> {
    let directory = world.town(&entry.town);
    ensure_private_dir(&directory.signed())?;
    let record = directory.signed().join(format!("{seq:020}.json"));
    let bytes = serde_json::to_vec(signed).map_err(io::Error::other)?;
    same_or_new(&record, &bytes)?;
    write_files(&directory, entry)
}

/// Writes an entry's files, each new or already the same.
fn write_files(directory: &TownDir, entry: &Entry) -> io::Result<()> {
    let ledger = directory.ledger();
    for file in &entry.files {
        let path = file.path.split('/').fold(ledger.clone(), |path, name| path.join(name));
        if let Some(parent) = path.parent() {
            ensure_private_dir(parent)?;
        }
        same_or_new(&path, &file.bytes.0)?;
        read_only(&path)?;
    }
    Ok(())
}

/// Finishes the files of entries a node that stopped had kept but not yet
/// written, and says which entries could not be read.
pub fn finish(world: &WorldDir) -> io::Result<Vec<String>> {
    let mut problems = Vec::new();
    for town in world.towns()? {
        let directory = world.town(&town);
        for (seq, path) in numbered(&directory.signed())? {
            let finished = read_entry(&path).and_then(|(entry, _)| write_files(&directory, &entry));
            if let Err(error) = finished {
                problems.push(format!("entry {seq} of {town}: {error}"));
            }
        }
    }
    Ok(problems)
}

/// The number of the latest entry this machine has of `town`; 0 when none.
pub fn seq(world: &WorldDir, town: &TownName) -> io::Result<u64> {
    cursor(&world.town(town))
}

/// The entries of `town` this machine has after the one numbered `after`,
/// in order.
pub fn entries_after(world: &WorldDir, town: &TownName, after: u64) -> io::Result<Vec<(u64, Entry)>> {
    let mut entries = Vec::new();
    for (seq, path) in numbered(&world.town(town).signed())? {
        if seq > after {
            entries.push((seq, read_entry(&path)?.0));
        }
    }
    Ok(entries)
}

fn cursor(directory: &TownDir) -> io::Result<u64> {
    Ok(numbered(&directory.signed())?.last().map(|(seq, _)| *seq).unwrap_or(0))
}

/// Every entry this machine has of a town, in order, as its author claims
/// it: each was checked when it was taken.
fn read_all(directory: &TownDir) -> io::Result<Vec<(u64, Entry, Signed)>> {
    let mut entries = Vec::new();
    for (seq, path) in numbered(&directory.signed())? {
        let (entry, signed) = read_entry(&path)?;
        entries.push((seq, entry, signed));
    }
    Ok(entries)
}

fn read_entry(path: &Path) -> io::Result<(Entry, Signed)> {
    let invalid = |error: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()));
    let signed: Signed = serde_json::from_slice(&std::fs::read(path)?).map_err(|error| invalid(error.to_string()))?;
    let entry: Entry = signed.claim().map_err(|error| invalid(error.to_string()))?;
    Ok((entry, signed))
}

/// Writes `bytes` at `path` unless it is there already; a file there with
/// other bytes is an error, as nothing in a ledger changes.
fn same_or_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match publish_new(path, bytes, Permissions::Private) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if std::fs::read(path)? == bytes {
                Ok(())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} holds other bytes than its entry", path.display()),
                ))
            }
        }
        other => other,
    }
}

/// A ledger's files are copies to read; they are changed only by taking
/// another entry, which never touches them.
fn read_only(path: &Path) -> io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    if !permissions.readonly() {
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

/// This machine's key, made now if it has none.
pub fn own_key(world: &WorldDir) -> io::Result<SecretKey> {
    match SecretKey::load(&world.device_key()) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let key = SecretKey::generate()?;
            key.create(&world.device_key())?;
            Ok(key)
        }
        other => other,
    }
}

/// The id a World of one machine signs its ledgers under, chosen once.
fn alone_world(world: &WorldDir) -> io::Result<WorldId> {
    let path = world.path().join("alone.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let id = WorldId::generate()?;
            publish(&path, &serde_json::to_vec(&id).map_err(io::Error::other)?, Permissions::Private)?;
            Ok(id)
        }
        Err(error) => Err(error),
    }
}

fn numbered(directory: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if let Some(seq) = path.file_stem().and_then(|stem| stem.to_str()).and_then(|stem| stem.parse::<u64>().ok())
            && path.extension().is_some_and(|extension| extension == "json")
        {
            found.push((seq, path));
        }
    }
    found.sort();
    Ok(found)
}
