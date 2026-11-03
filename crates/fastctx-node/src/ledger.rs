//! This machine's copies of the projects' ledgers (`world-network.md`, 共享
//! 账本), kept in each project's directory (`fastctx_types::layout::TownDir`).
//!
//! What counts is the signed entry; the files are what it says, written for
//! residents to read with the tools they have. Each entry taken is kept in
//! its signed form first, then its files, so a node that stopped in between
//! finds the entry and finishes its files when it starts again; a file
//! already there must hold the same bytes. Each entry is taken once, and
//! the files are left read-only. An entry whose signature does not check
//! out is noted with why (`refused.json`) and never taken.
//!
//! A project's ledger is kept at the hub of the account it was opened in
//! (`names::Place`). A computer on no hub keeps its projects itself, numbers
//! their entries as it writes them and signs them with its own key; when it
//! is first registered as a machine of an account, those projects are
//! carried into that account: each entry written again at the hub, in order,
//! under the name the computer has there.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::ids::HubId;
use fastctx_net::ledger::{self, Entry, File};
use fastctx_net::link::Link;
use fastctx_net::sign::{Bytes, SecretKey, Signed};
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::access::{HubAccount, TownId};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::{TownDir, WorldDir};
use fastctx_types::{MachineName, TownName};

use crate::Node;
use names::Place;

pub(crate) mod alone;
pub(crate) mod deletion;
pub(crate) mod letgo;
pub(crate) mod metadata;
pub(crate) mod names;
pub(crate) mod query;
pub(crate) mod syncing;
pub(crate) mod third;

pub use query::query;

/// Opens the project `town`: at the hub, in `owner`'s account, or kept here
/// when this computer is on no hub.
pub async fn open(
    node: &Arc<Node>,
    town: &TownName,
    owner: Option<HubAccount>,
    shared_on: Option<MachineName>,
) -> Result<(), Failure> {
    if names::place(&node.world, town).map_err(|error| Failure::new(error.to_string()))?.is_some() {
        return Err(Failure::new(format!("the project {town} exists already")));
    }
    let id = TownId::from_bytes(fastctx_net::sign::random::<8>().map_err(|error| Failure::new(error.to_string()))?);
    let place = match owner {
        Some(owner) => {
            let link = node
                .link_of(&owner.hub)
                .ok_or_else(|| Failure::new(format!("this computer is not on the hub {}", owner.hub)))?;
            if shared_on.is_some() && !link.view().shared_records {
                return Err(Failure::not_run("update the hub to create projects with whole-machine record ownership"));
            }
            let change = fastctx_net::wire::Change::OpenProject { town: id.clone(), name: town.clone(), shared_on };
            link.manage(&owner.account, change).await.map_err(Failure::new)?;
            Place { hub: owner.hub, town: id }
        }
        None => {
            if node.connected_anywhere() {
                return Err(Failure::new(
                    "a project is opened in an account; say which, as this computer is logged in to some",
                ));
            }
            Place { hub: alone(&node.world).map_err(|error| Failure::new(error.to_string()))?, town: id }
        }
    };
    let _syncing = node.syncing.lock().await;
    if names::place(&node.world, town)
        .map_err(|error| Failure::new(error.to_string()))?
        .is_some_and(|current| current != place)
    {
        return Err(Failure::new(format!(
            "the project {town} was assigned to another ledger while it was being opened"
        )));
    }
    names::set_place(&node.world, town, &place).map_err(|error| Failure::new(format!("recording {town}: {error}")))
}

/// Writes an entry into `town`'s ledger: `files`, at paths relative to the
/// ledger, and for a version of the project's record the machines it spans
/// from then on. Returns once this machine has the accepted entry, including
/// when it wrote on behalf of a device into a project that does not span it.
pub async fn write(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
) -> Result<(), Failure> {
    write_for(node, town, files, machines, None).await
}

pub(crate) async fn write_memory(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<File>,
    account: Option<fastctx_types::AccountName>,
) -> Result<(), Failure> {
    write_for(node, town, files.into_iter().map(|file| (file.path, file.bytes.0)).collect(), None, account).await
}

async fn write_for(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
    account: Option<fastctx_types::AccountName>,
) -> Result<(), Failure> {
    for (path, _) in &files {
        ledger::check_path(path).map_err(Failure::new)?;
    }
    let files: Vec<File> = files.into_iter().map(|(path, bytes)| File { path, bytes: Bytes(bytes) }).collect();
    let place = names::place(&node.world, town)
        .map_err(|error| Failure::new(format!("reading {town}: {error}")))?
        .ok_or_else(|| Failure::new(format!("there is no project {town} on this machine")))?;
    match node.link_of(&place.hub) {
        Some(link) => {
            let entry = link.entry(&place.town, machines, files);
            check_size(&entry).map_err(Failure::new)?;
            let signed = link.sign_entry(&entry);
            let accepted = match account {
                Some(account) if fastctx_net::topic_history::has_history(&entry.files) => {
                    link.history_write(&account, &place.town, signed).await
                }
                Some(account) => link.memory_write(&account, &place.town, signed).await,
                None => link.ledger_write(&place.town, signed).await,
            }
            .map_err(Failure::new)?;
            if link.view().member_keeps_project(&link.view().me, &place.town) {
                take(node, &link, town, &place.town, accepted).await.map_err(Failure::new)
            } else {
                metadata::refresh(node, &link, Some(&place.town)).await.map_err(Failure::new)
            }
        }
        None if place.hub == alone(&node.world).map_err(|error| Failure::new(error.to_string()))? => {
            let (world, here, town) = (node.world.clone(), node.machine(), town.clone());
            let _writing = node.syncing.lock().await;
            if names::place(&world, &town).map_err(|error| Failure::new(error.to_string()))?.as_ref() != Some(&place) {
                return Err(Failure::new(
                    "the project's ledger moved while the write was being prepared; read it again",
                ));
            }
            let writer = node.clone();
            fastctx_platform::task::spawn_blocking(move || {
                alone::write(&writer.alone_ledgers, &world, &here, &town, &place, files, machines)
            })
            .await
            .map_err(|error| Failure::new(format!("writing the ledger: {error}")))?
            .map_err(Failure::new)?;
            // The projects as the interfaces show them may have changed.
            node.shown_changed();
            Ok(())
        }
        None => Err(Failure::new(format!(
            "the project {town} is kept on the hub {}, which this computer is not logged in to",
            place.hub
        ))),
    }
}

/// Checks the encoded multipart limit before any files are kept.
fn check_size(entry: &Entry) -> Result<(), String> {
    match ledger::fits(entry) {
        Ok(_) => Ok(()),
        Err(over) => {
            let limit = fastctx_net::wire::PARTS_AT_MOST;
            Err(format!(
                "the ledger entry is {} bytes as it travels; the limit is {limit} bytes, including its signature and message encoding",
                limit + over
            ))
        }
    }
}

/// Takes the ledgers available to this machine's accounts, regardless of
/// the projects' work locations. One sync runs at a time.
pub async fn sync(node: &Arc<Node>, hub: &HubId, town: Option<TownId>) -> Result<(), String> {
    let Some(link) = node.link_of(hub) else { return Ok(()) };
    // A managed machine keeps no project's records: it acts for no one, and
    // the hub gives it none (`world-network.md`, 受管机器).
    if link.view().me().is_some_and(|me| me.managed) {
        return Ok(());
    }
    let mut problems = Vec::new();
    for (id, latest) in link.ledger_catalog().await? {
        if !link.view().member_keeps_project(&link.view().me, &id) {
            continue;
        }
        if town.as_ref().is_some_and(|town| *town != id) {
            continue;
        }
        let name = {
            let _syncing = node.syncing.lock().await;
            let (naming, link, id) = (node.clone(), link.clone(), id.clone());
            fastctx_platform::task::blocking(move || local_name(&naming, &link, &id)).await
        };
        match name {
            Ok(name) => {
                if let Err(error) = take(node, &link, &name, &id, latest).await {
                    problems.push(error);
                }
            }
            Err(error) => problems.push(error),
        }
    }
    if let Err(error) = metadata::refresh(node, &link, town.as_ref()).await {
        problems.push(error);
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

/// The short name this machine keeps the project `id` of `link`'s hub
/// under, recording where it is kept the first time.
pub(crate) fn local_name(node: &Node, link: &Link, id: &TownId) -> Result<TownName, String> {
    let view = link.view();
    let project = view.project(id).ok_or_else(|| format!("the hub {} names no project {id}", link.hub_name()))?;
    let place = Place { hub: link.membership().id.clone(), town: id.clone() };
    match names::place(&node.world, &project.name).map_err(|error| format!("reading {}: {error}", project.name))? {
        Some(kept) if kept == place => {}
        Some(kept) => {
            return Err(format!(
                "this machine keeps another project named {} (of the hub {}); the hub {}'s is not taken here",
                project.name,
                kept.hub,
                link.hub_name()
            ));
        }
        None => names::set_place(&node.world, &project.name, &place)
            .map_err(|error| format!("recording {}: {error}", project.name))?,
    }
    Ok(project.name.clone())
}

/// Takes `town`'s entries through `latest`.
pub(crate) async fn take(
    node: &Arc<Node>,
    link: &Arc<Link>,
    name: &TownName,
    town: &TownId,
    latest: u64,
) -> Result<(), String> {
    if !link.view().member_keeps_project(&link.view().me, town) {
        return Err("an assisting host keeps only the project index".into());
    }
    let directory = node.world.town(name);
    let mut after = {
        let _syncing = node.syncing.lock().await;
        let (checking, link, name, town, directory) =
            (node.clone(), link.clone(), name.clone(), town.clone(), directory.clone());
        fastctx_platform::task::blocking(move || -> Result<u64, String> {
            check_place(&checking, &link, &name, &town)?;
            cursor(&directory, &refused(&directory).map_err(|error| error.to_string())?)
                .map_err(|error| format!("reading {name}'s ledger: {error}"))
        })
        .await?
    };
    let changed = after < latest;
    let mut last_page = None;
    while after < latest {
        let (items, more, through) = link.ledger_projection(town, after).await?;
        if items.is_empty() && through <= after {
            return Err(format!("the hub promised {latest} entries of {name}, but stopped after {after}"));
        }
        let _syncing = node.syncing.lock().await;
        // The page goes into the project's files on the blocking pool.
        after = {
            let (keeping, link, name, town, directory) =
                (node.clone(), link.clone(), name.clone(), town.clone(), directory.clone());
            fastctx_platform::task::blocking(move || -> Result<u64, String> {
                check_place(&keeping, &link, &name, &town)?;
                let mut refused = refused(&directory).map_err(|error| error.to_string())?;
                let mut after = cursor(&directory, &refused).map_err(|error| error.to_string())?;
                for taken in items {
                    let seq = taken.seq;
                    // Another reader may have committed this prefix while we fetched it.
                    if seq <= after {
                        continue;
                    }
                    if !link.view().shared_records && seq != after + 1 {
                        return Err(format!("the hub gave entry {seq} of {name} after {after}"));
                    }
                    match (taken.entry, taken.refused) {
                        (Some(entry), None) => apply(&keeping.world, &name, seq, &entry, &taken.signed)
                            .map_err(|error| format!("keeping entry {seq} of {name}: {error}"))?,
                        (_, reason) => {
                            let reason = reason.unwrap_or_else(|| "it cannot be read".to_string());
                            keeping.log.line(format_args!("refused from the ledger: entry {seq} of {name}: {reason}"));
                            refused.insert(seq, reason);
                            set_refused(&directory, &refused)
                                .map_err(|error| format!("keeping {name}'s ledger: {error}"))?;
                        }
                    }
                    after = seq;
                }
                let after = after.max(through);
                ensure_private_dir(directory.path()).map_err(|error| error.to_string())?;
                fastctx_net::link::state::write(&directory.path().join("projection-through.json"), &after)
                    .map_err(|error| error.to_string())?;
                Ok(after)
            })
            .await?
        };
        if !more && after < latest {
            return Err(format!("the hub ended {name}'s ledger at {after}, before its promised {latest}"));
        }
        if after >= latest {
            // A reader that sees the final cursor also sees its projection.
            last_page = Some(_syncing);
        }
    }
    let _syncing = match last_page {
        Some(guard) => guard,
        None => node.syncing.lock().await,
    };
    check_place(node, link, name, town)?;
    if changed {
        crate::context::invalidations::project(node, name);
    }
    node.memory_changed();
    Ok(())
}

/// Rechecks local identity and current replication rights after every network wait.
fn check_place(node: &Node, link: &Link, name: &TownName, town: &TownId) -> Result<(), String> {
    let expected = Place { hub: link.membership().id.clone(), town: town.clone() };
    if names::place(&node.world, name).map_err(|error| error.to_string())?.as_ref() != Some(&expected) {
        return Err(format!("{name}'s ledger identity changed during synchronization"));
    }
    if !link.view().member_keeps_project(&link.view().me, town) {
        return Err("project replication permission ended during the read".into());
    }
    Ok(())
}

/// After this computer logged in to the hub `hub`: the projects it kept
/// while on no hub go into the account it is registered under there, and
/// what the hub has for it is taken.
pub fn joined(node: &Arc<Node>, hub: &HubId) {
    syncing::changed(node, hub, None);
}

/// Carries the projects this computer kept on no hub into the account it is
/// registered under at `hub`: each entry written to the hub again, in
/// order, by this machine under the name it has now. Returns what could
/// not be carried.
pub(crate) async fn carry(node: &Arc<Node>, hub: &HubId) -> Vec<String> {
    let Some(link) = node.link_of(hub) else { return Vec::new() };
    if let Err(error) = link.refresh().await {
        return vec![format!("asking the hub: {error}")];
    }
    let Some(home) = link.view().me().and_then(|me| me.home.clone()) else { return Vec::new() };
    let world = node.world.clone();
    let read = fastctx_platform::task::blocking(move || -> Result<_, String> {
        let alone = alone(&world).map_err(|error| format!("reading this computer's own id: {error}"))?;
        let towns = world.towns().map_err(|error| format!("reading the projects: {error}"))?;
        let key = own_key(&world).map_err(|error| format!("this computer's key: {error}"))?;
        Ok((alone, towns, key))
    })
    .await;
    let (alone, towns, key) = match read {
        Ok(read) => read,
        Err(problem) => return vec![problem],
    };
    let mut problems = Vec::new();
    for town in towns {
        let snapshot = node.syncing.lock().await;
        let directory = node.world.town(&town);
        let read = {
            let (world, town, directory, alone) = (node.world.clone(), town.clone(), directory.clone(), alone.clone());
            fastctx_platform::task::blocking(move || -> Result<Option<_>, String> {
                let place = match names::place(&world, &town) {
                    Ok(Some(place)) if place.hub == alone => place,
                    Ok(_) => return Ok(None),
                    Err(error) => return Err(format!("reading {town}: {error}")),
                };
                let entries =
                    read_all(&world, &directory).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
                Ok(Some((place, entries)))
            })
            .await
        };
        let (place, entries) = match read {
            Ok(Some(read)) => read,
            Ok(None) => continue,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };
        drop(snapshot);
        if let Err(error) = carry_town(node, &link, &home, &town, &place.town, &entries, &key.public()).await {
            problems.push(error);
            continue;
        }
        let _syncing = node.syncing.lock().await;
        let unchanged = {
            let home_now = link.view().me().and_then(|me| me.home.clone());
            let (world, checking, place, directory, home, entries) =
                (node.world.clone(), town.clone(), place.clone(), directory.clone(), home.clone(), entries.clone());
            fastctx_platform::task::blocking(move || -> io::Result<bool> {
                if names::place(&world, &checking)?.as_ref() != Some(&place) || home_now.as_ref() != Some(&home) {
                    return Ok(false);
                }
                let current = read_all(&world, &directory)?;
                Ok(current.len() == entries.len()
                    && current
                        .iter()
                        .zip(&entries)
                        .all(|(a, b)| a.0 == b.0 && a.2.data == b.2.data && a.2.sig == b.2.sig))
            })
            .await
        };
        match unchanged {
            Ok(true) => {}
            Ok(false) => {
                problems.push(format!(
                    "{town} changed while it was carried; its local ledger was retained for another pass"
                ));
                continue;
            }
            Err(error) => {
                problems.push(format!("rechecking {town} after carrying it: {error}"));
                continue;
            }
        }
        // The hub's copy is taken afresh: it may call this machine by
        // another name than the files here do.
        let moved = {
            let (world, town, place, directory) = (
                node.world.clone(),
                town.clone(),
                Place { hub: hub.clone(), town: place.town.clone() },
                directory.clone(),
            );
            fastctx_platform::task::blocking(move || {
                names::set_place(&world, &town, &place)
                    .and_then(|()| remove(&directory.signed()))
                    .and_then(|()| remove(&directory.ledger()))
            })
            .await
        };
        if let Err(error) = moved {
            problems.push(format!("letting go of {town}'s own ledger: {error}"));
            continue;
        }
        node.log.line(format_args!("carried {town} into the account {home}: {} entries", entries.len()));
    }
    problems
}

/// Entries this computer wrote, signed with `own`, are carried under the
/// name it has now; those of other machines, kept from the network's third
/// version (`third`), as their authors wrote them. Several computers may
/// carry one such project at once: each writes an entry only when the
/// hub has the ones before it, and one another wrote first is theirs.
async fn carry_town(
    node: &Arc<Node>,
    link: &Link,
    home: &fastctx_types::AccountName,
    town: &TownName,
    id: &TownId,
    entries: &[(u64, Entry, Signed)],
    own: &fastctx_net::sign::PublicKey,
) -> Result<(), String> {
    let here = node.machine();
    let open = fastctx_net::wire::Change::OpenProject {
        town: id.clone(),
        name: town.clone(),
        shared_on: link.view().shared_records.then(|| here.clone()),
    };
    if let Err(error) = link.manage(home, open).await
        && link.view().project(id).is_none_or(|project| project.name != *town)
    {
        return Err(format!("{town} was not carried: {error}"));
    }
    let latest = |catalog: Vec<(TownId, u64)>| {
        catalog.into_iter().find(|(town, _)| town == id).map(|(_, latest)| latest).unwrap_or(0)
    };
    // A carry cut short, or another computer's, wrote some of them already.
    let mut written = latest(link.ledger_catalog().await?);
    for (index, (_, entry, _)) in entries.iter().enumerate() {
        let index = index as u64;
        if index < written {
            continue;
        }
        // Each entry as the hub gets it: by this machine; one it wrote, under
        // the name it has now.
        let old = &entry.author;
        let ours = entry.key == *own && *old != here;
        let renamed = |name: &MachineName| if ours && name == old { here.clone() } else { name.clone() };
        let mut files = Vec::new();
        for file in &entry.files {
            let bytes = if ours {
                Bytes(crate::town::rename::file(&file.path, &file.bytes.0, old, &here)?)
            } else {
                file.bytes.clone()
            };
            files.push(File { path: file.path.clone(), bytes });
        }
        if ours {
            crate::town::rename::memory(&mut files, old, &here)?;
        }
        let mut carried =
            link.entry(id, entry.machines.as_ref().map(|machines| machines.iter().map(renamed).collect()), files);
        carried.at = entry.at;
        let written_entry = if fastctx_net::memory::has_memory(&carried.files) {
            link.memory_write(home, id, link.sign_entry(&carried)).await
        } else {
            link.ledger_write(id, link.sign_entry(&carried)).await
        };
        if let Err(error) = written_entry {
            written = latest(link.ledger_catalog().await?);
            if written > index {
                continue;
            }
            return Err(format!("carrying {town}: {error}"));
        }
        written = index + 1;
    }
    Ok(())
}

/// Removes a directory of ledger files, read-only as they are.
fn remove(directory: &Path) -> io::Result<()> {
    fn writable(path: &Path) -> io::Result<()> {
        let metadata = fastctx_platform::disk::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in fastctx_platform::disk::read_dir(path)? {
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
    fastctx_platform::disk::remove_dir_all(directory)
}

/// Takes entry `seq` of `town`: its signed form, then its files.
fn apply(world: &WorldDir, town: &TownName, seq: u64, entry: &Entry, signed: &Signed) -> io::Result<()> {
    let directory = world.town(town);
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
            let finished = read_entry(world, &path).and_then(|(entry, _)| write_files(&directory, &entry));
            if let Err(error) = finished {
                problems.push(format!("entry {seq} of {town}: {error}"));
            }
        }
    }
    Ok(problems)
}

/// The number of the latest entry this machine has of `town`; 0 when none.
pub fn seq(world: &WorldDir, town: &TownName) -> io::Result<u64> {
    let directory = world.town(town);
    if !fastctx_platform::disk::is_dir(directory.signed())
        && let Some(index) = metadata::read(world, town)?
    {
        return Ok(index.seq);
    }
    replica_seq(world, town)
}

/// The entry of `town` numbered `seq`, when this machine has it.
pub(crate) fn entry(world: &WorldDir, town: &TownName, seq: u64) -> io::Result<Option<Entry>> {
    let directory = world.town(town).signed();
    let path = directory.join(format!("{seq:020}.json"));
    let path = match fastctx_platform::disk::exists(&path)? {
        true => path,
        false => match numbered(&directory)?.into_iter().find(|(number, _)| *number == seq) {
            Some((_, path)) => path,
            None => return Ok(None),
        },
    };
    Ok(Some(read_entry(world, &path)?.0))
}

/// The full copy's cursor; an index-only cache is never a replicated entry.
pub(crate) fn replica_seq(world: &WorldDir, town: &TownName) -> io::Result<u64> {
    let directory = world.town(town);
    cursor(&directory, &refused(&directory)?)
}

/// The entries of `town` this machine has after the one numbered `after`,
/// in order.
pub fn entries_after(world: &WorldDir, town: &TownName, after: u64) -> io::Result<Vec<(u64, Entry)>> {
    let mut entries = Vec::new();
    for (seq, path) in numbered(&world.town(town).signed())? {
        if seq > after {
            entries.push((seq, read_entry(world, &path)?.0));
        }
    }
    Ok(entries)
}

/// The last entry taken or refused.
fn cursor(directory: &TownDir, refused: &BTreeMap<u64, String>) -> io::Result<u64> {
    let taken = numbered(&directory.signed())?.last().map(|(seq, _)| *seq).unwrap_or(0);
    let through: u64 = fastctx_net::link::state::read_or(&directory.path().join("projection-through.json"), 0)?;
    Ok(taken.max(refused.keys().next_back().copied().unwrap_or(0)).max(through))
}

fn refused(directory: &TownDir) -> io::Result<BTreeMap<u64, String>> {
    fastctx_net::link::state::read_or(&directory.path().join("refused.json"), BTreeMap::new())
}

fn set_refused(directory: &TownDir, refused: &BTreeMap<u64, String>) -> io::Result<()> {
    ensure_private_dir(directory.path())?;
    fastctx_net::link::state::write(&directory.path().join("refused.json"), refused)
}

/// Every entry this machine has of a project, in order, as its author
/// claims it: each was checked when it was taken.
fn read_all(world: &WorldDir, directory: &TownDir) -> io::Result<Vec<(u64, Entry, Signed)>> {
    let mut entries = Vec::new();
    for (seq, path) in numbered(&directory.signed())? {
        let (entry, signed) = read_entry(world, &path)?;
        entries.push((seq, entry, signed));
    }
    Ok(entries)
}

fn read_entry(world: &WorldDir, path: &Path) -> io::Result<(Entry, Signed)> {
    let invalid = |error: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()));
    let signed: Signed =
        serde_json::from_slice(&fastctx_platform::disk::read(path)?).map_err(|error| invalid(error.to_string()))?;
    let claimed: Entry = signed.claim().map_err(|error| invalid(error.to_string()))?;
    let town = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(|name| TownName::try_from(name).ok())
        .ok_or_else(|| invalid("the signed entry is outside a project directory".into()))?;
    let place = names::place(world, &town)?.ok_or_else(|| invalid("the project has no ledger identity".into()))?;
    if claimed.hub != place.hub || claimed.town != place.town {
        return Err(invalid("the signed entry belongs to another project or hub".into()));
    }
    // Network-v3 imports used a zero author key but were signed by this device.
    let entry = if claimed.key == fastctx_net::sign::PublicKey::from_bytes([0; 32])
        && !fastctx_net::memory::has_memory(&claimed.files)
    {
        let key = SecretKey::load(&world.device_key())?;
        let entry: Entry = signed
            .open(&key.public(), fastctx_net::sign::Domain::Ledger)
            .map_err(|error| invalid(error.to_string()))?;
        for file in &entry.files {
            ledger::check_path(&file.path).map_err(&invalid)?;
        }
        entry
    } else {
        ledger::open(&signed, &claimed.hub, &claimed.town).map_err(invalid)?
    };
    Ok((entry, signed))
}

/// Writes `bytes` at `path` unless it is there already; a file there with
/// other bytes is an error, as nothing in a ledger changes.
fn same_or_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match publish_new(path, bytes, Permissions::Private) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if fastctx_platform::disk::read(path)? == bytes {
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
    let mut permissions = fastctx_platform::disk::metadata(path)?.permissions();
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

/// The id a computer on no hub keeps its own projects under, chosen once.
pub(crate) fn alone(world: &WorldDir) -> io::Result<HubId> {
    let path = world.path().join("alone.json");
    match fastctx_platform::disk::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let id = fastctx_net::ids::new_hub_id()?;
            publish(&path, &serde_json::to_vec(&id).map_err(io::Error::other)?, Permissions::Private)?;
            Ok(id)
        }
        Err(error) => Err(error),
    }
}

fn numbered(directory: &Path) -> io::Result<Vec<(u64, PathBuf)>> {
    let mut found = Vec::new();
    let entries = match fastctx_platform::disk::read_dir(directory) {
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
