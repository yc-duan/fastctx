//! This machine's copies of the ledgers of the towns it spans
//! (`world-network.md`, 共享账本), kept in each town's directory
//! (`fastctx_types::layout::TownDir`).
//!
//! What counts is the signed entry; the files are what it says, written for
//! residents to read with the tools they have. The hub keeps entries
//! sealed (`fastctx_net::ledger::sealed`); this machine keeps what it takes
//! from the hub as it came (`items`) and opens each entry it has a key for
//! into its town (`open`): its signed form first, then its files, so a node
//! that stopped in between finds the entry and finishes its files when it
//! starts again; a file already there must hold the same bytes. Each entry
//! is taken once, and the files are left read-only.
//!
//! A World of one machine keeps its ledgers itself, numbers entries as it
//! writes them, and signs them with its own key, made the first time it
//! needs one; nothing leaves the machine, so nothing is sealed. When it
//! joins a World, the towns it kept alone are carried into it: each entry
//! written again, in order, signed and sealed for that World.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fastctx_net::ledger::{self, Entry, File, Item, TownId};
use fastctx_net::link::Link;
use fastctx_net::sign::{Bytes, SecretKey, Signed};
use fastctx_net::trust::WorldId;
use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::{TownDir, WorldDir};
use fastctx_types::{MachineName, TownName};
use jiff::Timestamp;

use crate::Node;

mod items;
pub(crate) mod names;
mod open;
pub(crate) mod query;
mod share;

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
            let id = names::require(node, town).map_err(Failure::new)?;
            let naming = node.naming.read().expect("never poisoned").clone();
            let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
            let entry = Entry {
                world: link.membership().world.clone(),
                town: town.clone(),
                author: node.machine(),
                at: Timestamp::now(),
                machines: machines.clone(),
                files,
            };
            check_size(&entry, readers(&link, &id)).map_err(Failure::new)?;
            let inner = link.sign_entry(&entry);
            let item = link.seal_entry(&id, naming.as_ref(), &paths, machines, &inner).map_err(Failure::new)?;
            let accepted = link.ledger_write(item).await.map_err(Failure::new)?;
            let _syncing = node.syncing.lock().await;
            if take(node, &link, &id, accepted, true).await.map_err(Failure::new)? {
                Ok(())
            } else {
                Err(Failure::new(format!(
                    "the hub accepted item {accepted} of {town}, but its connection ended before the local copy arrived"
                )))
            }
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

/// How many members an entry of `town` is sealed for.
fn readers(link: &Link, town: &TownId) -> usize {
    fastctx_net::ledger::sealed::readers(&link.members().0, town).len()
}

/// The room an entry of `town` has before it no longer fits in one message,
/// as `ledger::fits` measures it for the readers it is sealed for here.
pub fn fits(node: &Node, town: &TownName, entry: &Entry) -> Result<usize, usize> {
    let readers = match (node.link(), names::town_id(node, town)) {
        (Some(link), Some(id)) => readers(&link, &id),
        _ => 1,
    };
    ledger::fits(entry, readers)
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
    check_size(&entry, 1)?;
    let signed = ledger::sign(&key, &entry);
    let seq = entries.last().map(|(seq, _, _)| *seq).unwrap_or(0) + 1;
    apply(world, seq, &entry, &signed).map_err(|error| format!("keeping the entry: {error}"))
}

/// Both standalone and connected writers measure the complete wire item
/// before publishing any of its files.
fn check_size(entry: &Entry, readers: usize) -> Result<(), String> {
    match ledger::fits(entry, readers) {
        Ok(_) => Ok(()),
        Err(over) => {
            let limit = fastctx_net::wire::MAX_CONTROL_FRAME;
            Err(format!(
                "the ledger entry is {} bytes once sealed; the limit is {limit} bytes (256 KiB), including its signature, keys and message encoding",
                limit + over
            ))
        }
    }
}

/// Takes what the hub has that this machine does not, for `town` or for
/// every town it spans, after carrying in the towns kept while alone. A
/// machine with management rights takes every town, and seals keys again
/// for the readers that lack them. One sync runs at a time.
pub async fn sync(node: &Arc<Node>, town: Option<TownName>) -> Result<(), String> {
    match town {
        Some(town) => {
            let Some(link) = node.link() else { return Ok(()) };
            names::load(node, &link);
            let id = names::require(node, &town)?;
            sync_ids(node, Some(id)).await
        }
        None => sync_ids(node, None).await,
    }
}

/// As `sync`, for a town by its id.
pub async fn sync_ids(node: &Arc<Node>, town: Option<TownId>) -> Result<(), String> {
    let Some(link) = node.link() else { return Ok(()) };
    let _syncing = node.syncing.lock().await;
    names::load(node, &link);
    let mut problems = Vec::new();
    if let Err(error) = names::ensure(node, &link).await {
        problems.push(format!("writing the World's naming key: {error}"));
    }
    let mut at_hub: BTreeMap<TownId, (u64, bool)> =
        link.ledger_towns().await?.into_iter().map(|(town, latest)| (town, (latest, false))).collect();
    if link.me().is_some_and(|me| me.manage) {
        for (town, latest) in link.ledger_catalog().await? {
            at_hub.entry(town).or_insert((latest, true));
        }
    }
    // The World's own ledger first: the naming key it holds tells which
    // town every other entry is of.
    let mut order: Vec<(TownId, (u64, bool))> = at_hub.into_iter().collect();
    order.sort_by_key(|(town, _)| *town != TownId::world());
    for (id, (latest, query)) in order {
        if town.as_ref().is_some_and(|town| *town != id && id != TownId::world()) {
            continue;
        }
        if let Err(error) = take(node, &link, &id, latest, query).await {
            problems.push(error);
        }
    }
    if town.is_none() {
        problems.extend(carry(node, &link).await);
    }
    problems.extend(share::reconcile(node, &link).await);
    if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
}

/// Takes `town`'s items through `latest` and opens what this machine can.
/// False means an explicit query's control connection ended; refusals,
/// invalid items and disk errors fail.
pub(crate) async fn take(
    node: &Arc<Node>,
    link: &Arc<Link>,
    town: &TownId,
    latest: u64,
    query: bool,
) -> Result<bool, String> {
    let store = items::Items::new(&node.world);
    let mut after = store.cursor(town).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
    let mut state = store.state(town).map_err(|error| format!("reading {town}'s ledger: {error}"))?;
    while after < latest {
        let page =
            if query { link.try_ledger_query(town, after).await? } else { Some(link.ledger_read(town, after).await?) };
        let Some((items, more)) = page else { return Ok(false) };
        if items.is_empty() {
            return Err(format!("the hub promised {latest} items of {town}, but stopped after {after}"));
        }
        for taken in items {
            let seq = taken.seq;
            if seq != after + 1 {
                return Err(format!("the hub gave item {seq} of {town} after {after}"));
            }
            store.keep(town, seq, &taken.signed).map_err(|error| format!("keeping item {seq} of {town}: {error}"))?;
            match (&taken.item, taken.refused) {
                (Item::Entry(_), Some(reason)) => {
                    node.log.line(format_args!("refused from the ledger: item {seq} of {town}: {reason}"));
                    state.refused.insert(seq, reason);
                }
                (Item::Entry(_), None) => {
                    state.unopened.insert(seq);
                }
                // A copy sealed for this machine may open an entry waiting;
                // one that does not open it is tried and passed over.
                (Item::Share(_), _) => {}
            }
            after = seq;
        }
        store.set_state(town, &state).map_err(|error| format!("keeping {town}'s ledger: {error}"))?;
        if !more && after < latest {
            return Err(format!("the hub ended {town}'s ledger at {after}, before its promised {latest}"));
        }
    }
    let (node_, link_, town_) = (node.clone(), link.clone(), town.clone());
    let problems = tokio::task::spawn_blocking(move || open::town(&node_, &link_, &town_))
        .await
        .map_err(|error| format!("opening {town}'s entries: {error}"))??;
    for problem in problems {
        node.log.line(format_args!("refused from the ledger: {problem}"));
    }
    Ok(true)
}

/// Carries only towns created under this node's standalone World identity:
/// its entries are written to the hub again, in order, by this machine and
/// for this World, and the copies signed for the old one are let go. A town
/// the hub has some of these entries of already, from a carry cut short, is
/// carried on from there. Returns what could not be carried.
async fn carry(node: &Arc<Node>, link: &Arc<Link>) -> Vec<String> {
    let world = link.membership().world.clone();
    let alone = match std::fs::read(node.world.path().join("alone.json")) {
        Ok(bytes) => match serde_json::from_slice::<WorldId>(&bytes) {
            Ok(alone) if alone != world => alone,
            Ok(_) => return Vec::new(),
            Err(error) => return vec![format!("reading the standalone World identity: {error}")],
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => return vec![format!("reading the standalone World identity: {error}")],
    };
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
        let own = entries.iter().filter(|(_, entry, _)| entry.world == alone).count();
        if own == 0 {
            continue;
        }
        if own < entries.len() {
            problems.push(format!(
                "{town} mixes standalone entries with another World; it is left as it is in {}",
                directory.path().display()
            ));
            continue;
        }
        if let Err(error) = carry_town(node, link, &world, &here, &town, &entries).await {
            problems.push(error);
            continue;
        }
        // The World's copy is taken afresh: it may call this machine by
        // another name than the files here do.
        if let Err(error) = remove(&directory.signed()).and_then(|()| remove(&directory.ledger())) {
            problems.push(format!("letting go of {town}'s ledger of its old World: {error}"));
            continue;
        }
        // The World's copy comes back as the hub tells this machine of the
        // items just written.
        node.log.line(format_args!("carried {town} into the World {world}: {} entries", entries.len()));
    }
    problems
}

async fn carry_town(
    node: &Arc<Node>,
    link: &Link,
    world: &WorldId,
    here: &MachineName,
    town: &TownName,
    entries: &[(u64, Entry, Signed)],
) -> Result<(), String> {
    let taken = format!("a town named {town} is in this World already; this machine's own {town} was not carried");
    let id = names::require(node, town)?;
    let naming = node.naming.read().expect("never poisoned").clone();
    for (_, entry, _) in entries {
        // Each entry as the World gets it: by this machine, under the name
        // it has now.
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
        let carried = Entry {
            world: world.clone(),
            town: town.clone(),
            author: here.clone(),
            at: entry.at,
            machines: entry.machines.as_ref().map(|machines| machines.iter().map(renamed).collect()),
            files,
        };
        let paths: Vec<String> = carried.files.iter().map(|file| file.path.clone()).collect();
        let inner = link.sign_entry(&carried);
        let item = link.seal_entry(&id, naming.as_ref(), &paths, carried.machines.clone(), &inner)?;
        match link.ledger_write(item).await {
            Ok(_) => {}
            Err(error) if error.contains("written twice") => {
                // Carried before by a carry cut short, or another machine's
                // town of that name: what is there says which.
                if !carried_before(node, link, &id, &carried).await? {
                    return Err(taken);
                }
            }
            Err(error) => return Err(format!("carrying {town}: {error}")),
        }
    }
    Ok(())
}

/// Whether the hub's ledger of `town` has `entry` as this machine carried it.
async fn carried_before(node: &Arc<Node>, link: &Link, town: &TownId, entry: &Entry) -> Result<bool, String> {
    let mut after = 0;
    loop {
        let (items, more) = link.ledger_query(town, after).await?;
        let shares: Vec<_> = items
            .iter()
            .filter_map(|taken| match &taken.item {
                Item::Share(share) => Some(share.clone()),
                Item::Entry(_) => None,
            })
            .collect();
        for taken in &items {
            after = taken.seq;
            let Item::Entry(sealed) = &taken.item else { continue };
            if sealed.author != node.machine() || taken.refused.is_some() {
                continue;
            }
            let Some((_, inner)) = link.open_sealed(sealed, &shares)? else { continue };
            if inner.claim::<Entry>().is_ok_and(|theirs| theirs.files == entry.files) {
                return Ok(true);
            }
        }
        if !more {
            return Ok(false);
        }
    }
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

/// The immutable intake identity, independent of the current machine name.
fn origin(directory: &TownDir) -> io::Result<Option<WorldId>> {
    numbered(&directory.signed())?.first().map(|(_, path)| read_entry(path).map(|(entry, _)| entry.world)).transpose()
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
