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
pub(crate) mod carry;
mod carry_source;
pub(crate) mod deletion;
pub(crate) mod letgo;
pub(crate) mod metadata;
pub(crate) mod names;
pub(crate) mod projected;
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
    names::set_place(&node.world, town, &place).map_err(|error| Failure::new(format!("recording {town}: {error}")))?;
    node.topics.changed(&format!("{}{town}", fastctx_types::topics::MAP_PREFIX));
    Ok(())
}

/// Writes an entry into `town`'s ledger: `files`, at paths relative to the
/// ledger, and for a version of the project's record the machines it spans
/// from then on, including on behalf of a device into a project that does not
/// span it. Returns once the hub accepted the entry (`write_for`).
pub async fn write(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
) -> Result<(), Failure> {
    write_for(node, town, files, machines, None).await
}

/// An entry the hub accepted, and whether this machine has taken it yet:
/// taking it comes after the entry is the hub's and can fail on its own.
pub(crate) struct Accepted {
    pub taken: Result<(), Failure>,
}

/// As `write`, but an entry the hub accepted is told as written whatever
/// became of taking it here, which the ledger's sync does later.
pub(crate) async fn write_accepted(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
) -> Result<Accepted, Failure> {
    accept_for(node, town, files, machines, None).await
}

pub(crate) async fn write_memory(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<File>,
    account: Option<fastctx_types::AccountName>,
) -> Result<(), Failure> {
    write_for(node, town, files.into_iter().map(|file| (file.path, file.bytes.0)).collect(), None, account).await
}

/// Writes an entry: written once the hub accepted it. Taking it here comes
/// after, and is waited for a while (`take_latest`); one this machine has not
/// taken by then is for the ledger's sync to catch up on, not a failed write
/// to try again (which would write it twice).
async fn write_for(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
    account: Option<fastctx_types::AccountName>,
) -> Result<(), Failure> {
    if let Err(failure) = accept_for(node, town, files, machines, account).await?.taken {
        node.log.line(format_args!(
            "project {town}: its hub accepted an entry; this machine has not taken it yet: {failure}"
        ));
    }
    Ok(())
}

async fn accept_for(
    node: &Arc<Node>,
    town: &TownName,
    files: Vec<(String, Vec<u8>)>,
    machines: Option<Vec<MachineName>>,
    account: Option<fastctx_types::AccountName>,
) -> Result<Accepted, Failure> {
    for (path, _) in &files {
        ledger::check_path(path).map_err(Failure::new)?;
    }
    let files: Vec<File> = files.into_iter().map(|(path, bytes)| File { path, bytes: Bytes(bytes) }).collect();
    let place = names::place(&node.world, town)
        .map_err(|error| Failure::new(format!("reading {town}: {error}")))?
        .ok_or_else(|| Failure::new(format!("there is no project {town} on this machine")))?;
    if carry::writing(&node.world, town).map_err(|error| Failure::new(error.to_string()))? {
        return Err(Failure::not_run("the project's ledger is being imported; try writing again shortly"));
    }
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
            let taken = if link.view().member_keeps_project(&link.view().me, &place.town) {
                // The hub accepted the entry: its projection is on its way
                // here, and is waited for.
                take_latest(node, &link, town, &place.town, accepted, true).await.map_err(Failure::new)
            } else {
                metadata::refresh(node, &link, Some(&place.town)).await.map_err(Failure::new)
            };
            Ok(Accepted { taken })
        }
        None if place.hub == alone(&node.world).map_err(|error| Failure::new(error.to_string()))? => {
            let (world, here, town) = (node.world.clone(), node.machine(), town.clone());
            let _writing = node.syncing.lock().await;
            if carry::writing(&world, &town).map_err(|error| Failure::new(error.to_string()))? {
                return Err(Failure::not_run("the project's ledger is being imported; try writing again shortly"));
            }
            if names::place(&world, &town).map_err(|error| Failure::new(error.to_string()))?.as_ref() != Some(&place) {
                return Err(Failure::new(
                    "the project's ledger moved while the write was being prepared; read it again",
                ));
            }
            let writer = node.clone();
            fastctx_platform::task::spawn_blocking(move || {
                let _map_changed = writer.topics.writing(format!("{}{}", fastctx_types::topics::MAP_PREFIX, town));
                alone::write(&writer.alone_ledgers, &world, &here, &town, &place, files, machines)
            })
            .await
            .map_err(|error| Failure::new(format!("writing the ledger: {error}")))?
            .map_err(Failure::new)?;
            // A local creation can finish after the first login's discovery pass.
            for link in node.links() {
                joined(node, link.hub());
            }
            // The projects as the interfaces show them may have changed.
            node.shown_changed();
            Ok(Accepted { taken: Ok(()) })
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
    // What a pass takes, or lets go of, is no longer known whole.
    query::unknown(Some(hub), None);
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
            fastctx_platform::task::blocking(move || sync_name(&naming, &link, &id)).await
        };
        match name {
            Ok(Some(name)) => {
                // A projection not here yet is taken by a later pass, which
                // this worker comes to by itself: a pass that waited for it
                // would hold up all that goes after it here.
                if let Err(error) = take_latest(node, &link, &name, &id, latest, false).await {
                    problems.push(error);
                }
            }
            Ok(None) => {}
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
    sync_name(node, link, id)?.ok_or_else(|| "the project's standalone ledger is being imported".into())
}

fn sync_name(node: &Node, link: &Link, id: &TownId) -> Result<Option<TownName>, String> {
    let view = link.view();
    let project = view.project(id).ok_or_else(|| format!("the hub {} names no project {id}", link.hub_name()))?;
    let place = Place { hub: link.membership().id.clone(), town: id.clone() };
    crate::record_migration::archive::with_import_gate(|| {
        if carry::writing(&node.world, &project.name).map_err(|error| error.to_string())? {
            return Ok(None);
        }
        let kept = match names::place(&node.world, &project.name)
            .map_err(|error| format!("reading {}: {error}", project.name))?
        {
            Some(kept) => kept,
            None => {
                let kept = names::claim(&node.world, &project.name, &place)
                    .map_err(|error| format!("recording {}: {error}", project.name))?;
                node.topics.changed(&format!("{}{}", fastctx_types::topics::MAP_PREFIX, project.name));
                kept
            }
        };
        if kept != place {
            return Err(format!(
                "this machine keeps another project named {} (of the hub {}); the hub {}'s is not taken here",
                project.name,
                kept.hub,
                link.hub_name()
            ));
        }
        Ok(Some(project.name.clone()))
    })
}

/// Takes `town`'s entries through `latest`: from the hub's projection for
/// the account this machine reads it as, where it writes one and the copy
/// here holds it (design §28.58); otherwise replicated as before. A
/// projection not here yet is waited for a while where `waits`: an entry the
/// hub just accepted is written to it and pulled here a moment after the
/// hub's answer, and the write that made it is not failed for that moment.
pub(crate) async fn take_latest(
    node: &Arc<Node>,
    link: &Arc<Link>,
    name: &TownName,
    town: &TownId,
    latest: u64,
    waits: bool,
) -> Result<(), String> {
    // One while for the projection to be here and to come up to `latest`.
    let until = std::time::Instant::now() + projected::WAITS;
    let kept_projected = || fastctx_platform::disk::is_dir(node.world.town(name).projected());
    let source = loop {
        let seen = {
            let (looking, link, town) = (node.clone(), link.clone(), town.clone());
            fastctx_platform::task::spawn_blocking(move || projected::source(&looking, &link, &town))
                .await
                .map_err(|error| error.to_string())?
        };
        match seen {
            projected::Seen::At(source) => break source,
            // A project of no entries waits for nothing.
            projected::Seen::Waiting { .. } if latest == 0 => return Ok(()),
            // The projection for the account and machines of now is not here:
            // what was kept of one for others goes now, not once that one
            // came, unless it was for the same account and machines all among
            // those it may work on now (codex-357de52a #2, third round #6,
            // fourth round #3). Never replicated meanwhile (third round #1).
            projected::Seen::Waiting { scope, machines } => {
                if kept_projected()
                    && !projected_readable(node, name, move |node, name| {
                        projected::still_readable(node, name, &scope, &machines)
                    })
                    .await?
                {
                    let_go_projected(node, name).await?;
                }
                if !waits || std::time::Instant::now() >= until {
                    return Err(format!("the hub's projection of {name}'s ledger is not here yet"));
                }
                tokio::time::sleep(projected::LOOKS).await;
            }
            // A project kept from its projection is never mixed with entries
            // replicated beside it.
            projected::Seen::Not if kept_projected() => {
                return Err(format!("the hub's projection of {name}'s ledger is not here yet"));
            }
            projected::Seen::Not => return take(node, link, name, town, latest).await,
            // Shared whole, it reads no projection: what it kept of one for
            // its owner alone goes, and the entries are replicated.
            projected::Seen::Whole => {
                if kept_projected() {
                    let_go_projected(node, name).await?;
                }
                return take(node, link, name, town, latest).await;
            }
        }
    };
    // Kept for another account, or machines it may no longer all work on:
    // gone before anything is taken, so nothing of it is read while the
    // projection is made again (codex-357de52a, fourth round #3).
    if kept_projected() {
        let looked = source.clone();
        if !projected_readable(node, name, move |node, name| projected::readable_for(node, name, &looked)).await? {
            let_go_projected(node, name).await?;
        }
    }
    if !link.view().member_keeps_project(&link.view().me, town) {
        return Err("an assisting host keeps only the project index".into());
    }
    let changed = projected::take(node, link, name, town, latest, source, until, waits).await?;
    let _syncing = node.syncing.lock().await;
    check_place(node, link, name, town)?;
    if changed {
        crate::context::invalidations::project(node, name);
    }
    node.memory_changed();
    Ok(())
}

/// Whether what is kept of `name` from a projection may be read on, as
/// `readable` says, looked at off the runtime.
async fn projected_readable(
    node: &Arc<Node>,
    name: &TownName,
    readable: impl FnOnce(&Node, &TownName) -> bool + Send + 'static,
) -> Result<bool, String> {
    let (looking, name) = (node.clone(), name.clone());
    fastctx_platform::task::spawn_blocking(move || readable(&looking, &name)).await.map_err(|error| error.to_string())
}

/// Lets go of what is kept of `name` from a projection it may no longer be
/// read from, with the ledgers' sync held.
async fn let_go_projected(node: &Arc<Node>, name: &TownName) -> Result<(), String> {
    let _syncing = node.syncing.lock().await;
    let (clearing, cleared) = (node.clone(), name.clone());
    fastctx_platform::task::blocking(move || projected::clear(&clearing, &cleared))
        .await
        .map_err(|error| format!("letting go of {name}'s ledger as it was projected before: {error}"))?;
    crate::context::invalidations::project(node, name);
    node.memory_changed();
    Ok(())
}

/// Takes `town`'s entries through `latest`, replicated from the hub.
async fn take(node: &Arc<Node>, link: &Arc<Link>, name: &TownName, town: &TownId, latest: u64) -> Result<(), String> {
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
                        (Some(entry), None) => {
                            let _map_changed =
                                keeping.topics.writing(format!("{}{}", fastctx_types::topics::MAP_PREFIX, name));
                            apply(&keeping.world, &name, seq, &entry, &taken.signed)
                                .map_err(|error| format!("keeping entry {seq} of {name}: {error}"))?;
                        }
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
    if carry::writing(&node.world, name).map_err(|error| error.to_string())? {
        return Err("the project's standalone ledger is still being archived".into());
    }
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
    write_files(&directory.ledger(), &entry.files)
}

/// Writes an entry's files under `ledger`, each new or already the same:
/// but an earlier release's pages, which are no more (beta.18).
fn write_files(ledger: &Path, files: &[File]) -> io::Result<()> {
    for file in files.iter().filter(|file| !removed(&file.path)) {
        let path = file.path.split('/').fold(ledger.to_path_buf(), |path, name| path.join(name));
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
pub(crate) fn finish(node: &Node) -> io::Result<Vec<String>> {
    let world = &node.world;
    let mut problems = Vec::new();
    for town in carry::towns(world)? {
        let _map_changed = node.topics.writing(format!("{}{}", fastctx_types::topics::MAP_PREFIX, town));
        if let Err(error) = carry::recover(world, &town, false) {
            problems.push(format!("finishing the import of {town}: {error}"));
            continue;
        }
        if carry::writing(world, &town)? {
            continue;
        }
        let directory = world.town(&town);
        // The pages an earlier release published among the project's files
        // go (beta.18). The jobs they ran stay: their records and output are
        // under the project's own `pages` (codex-e7b1552a R1).
        if let Err(error) = remove(&directory.ledger().join("pages")) {
            problems.push(format!("letting go of {town}'s pages: {error}"));
        }
        let kept = kept_files(&directory)?;
        let key = kept_key(world, &kept)?;
        for (seq, path, _) in kept {
            let finished =
                read_kept(world, &path, key.as_ref()).and_then(|kept| write_files(&directory.ledger(), &kept.files));
            if let Err(error) = finished {
                problems.push(format!("entry {seq} of {town}: {error}"));
            }
        }
    }
    Ok(problems)
}

/// Whether a ledger's file at `path` is of what is no more: the pages an
/// earlier release let agents publish as HTML (removed in beta.18).
fn removed(path: &str) -> bool {
    path.starts_with("pages/")
}

/// The files of an entry this machine keeps of a project: one its author
/// signed, or the hub's projection of one for the account this machine
/// reads the project as (design §28.58); but those of what is no more.
pub struct Kept {
    pub files: Vec<File>,
}

impl Kept {
    fn of(files: Vec<File>) -> Self {
        Self { files: files.into_iter().filter(|file| !removed(&file.path)).collect() }
    }
}

impl From<Entry> for Kept {
    fn from(entry: Entry) -> Self {
        Self::of(entry.files)
    }
}

impl From<ledger::Projection> for Kept {
    fn from(projection: ledger::Projection) -> Self {
        Self::of(projection.files)
    }
}

/// The entries this machine keeps of a project, in order, each with its
/// file and whether the hub projected it: a project taken from its
/// projection keeps those alone.
fn kept_files(directory: &TownDir) -> io::Result<Vec<(u64, PathBuf, bool)>> {
    if fastctx_platform::disk::is_dir(directory.projected()) {
        return Ok(numbered(&directory.projected())?.into_iter().map(|(seq, path)| (seq, path, true)).collect());
    }
    Ok(numbered(&directory.signed())?.into_iter().map(|(seq, path)| (seq, path, false)).collect())
}

/// The key what this machine keeps of a project's projection is checked
/// with, where `files` hold one (`projected::keep`).
fn kept_key(world: &WorldDir, files: &[(u64, PathBuf, bool)]) -> io::Result<Option<fastctx_net::sign::PublicKey>> {
    match files.iter().any(|(_, _, projected)| *projected) {
        true => Ok(Some(own_key(world)?.public())),
        false => Ok(None),
    }
}

/// The entry kept at `path`: one taken from the hub's projection, under
/// this machine's word `key`, checked as one it signs itself is, and as one
/// of the project it is kept in.
fn read_kept(world: &WorldDir, path: &Path, key: Option<&fastctx_net::sign::PublicKey>) -> io::Result<Kept> {
    let invalid = |error: String| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()));
    if let Some(key) = key {
        let signed: Signed =
            serde_json::from_slice(&fastctx_platform::disk::read(path)?).map_err(|error| invalid(error.to_string()))?;
        let kept: projected::KeptEntry =
            signed.open(key, fastctx_net::sign::Domain::LedgerKept).map_err(|error| invalid(error.to_string()))?;
        let town = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .and_then(|name| TownName::try_from(name).ok())
            .ok_or_else(|| invalid("the kept entry is outside a project directory".into()))?;
        if names::place(world, &town)?.map(|place| place.town) != Some(kept.town) {
            return Err(invalid("the kept entry is another project's".into()));
        }
        return Ok(kept.projection.into());
    }
    Ok(read_entry(world, path)?.0.into())
}

/// The number of the latest entry this machine has of `town`; 0 when none.
pub fn seq(world: &WorldDir, town: &TownName) -> io::Result<u64> {
    let directory = world.town(town);
    if !fastctx_platform::disk::is_dir(directory.signed())
        && !fastctx_platform::disk::is_dir(directory.projected())
        && let Some(index) = metadata::read(world, town)?
    {
        return Ok(index.seq);
    }
    replica_seq(world, town)
}

/// The entry of `town` numbered `seq`, when this machine has it.
pub(crate) fn entry(world: &WorldDir, town: &TownName, seq: u64) -> io::Result<Option<Kept>> {
    if let Some(entries) = carry_source::entries(world, town)? {
        return Ok(entries.into_iter().find(|(number, _)| *number == seq).map(|(_, entry)| entry.into()));
    }
    let kept = kept_files(&world.town(town))?;
    let key = kept_key(world, &kept)?;
    let Some((_, path, _)) = kept.into_iter().find(|(number, _, _)| *number == seq) else {
        return Ok(None);
    };
    Ok(Some(read_kept(world, &path, key.as_ref())?))
}

/// The full copy's cursor; an index-only cache is never a replicated entry.
pub(crate) fn replica_seq(world: &WorldDir, town: &TownName) -> io::Result<u64> {
    if let Some(entries) = carry_source::entries(world, town)? {
        return Ok(entries.last().map_or(0, |(seq, _)| *seq));
    }
    let directory = world.town(town);
    cursor(&directory, &refused(&directory)?)
}

/// The entries of `town` this machine has after the one numbered `after`,
/// in order.
pub fn entries_after(world: &WorldDir, town: &TownName, after: u64) -> io::Result<Vec<(u64, Kept)>> {
    if let Some(entries) = carry_source::entries(world, town)? {
        return Ok(entries
            .into_iter()
            .filter(|(seq, _)| *seq > after)
            .map(|(seq, entry)| (seq, entry.into()))
            .collect());
    }
    let kept = kept_files(&world.town(town))?;
    let key = kept_key(world, &kept)?;
    let mut entries = Vec::new();
    for (seq, path, _) in kept {
        if seq > after {
            entries.push((seq, read_kept(world, &path, key.as_ref())?));
        }
    }
    Ok(entries)
}

/// The signed entries of `town` this machine has after the one numbered
/// `after`, in order: those of a project kept on no hub, which this machine
/// signs itself.
pub(crate) fn signed_after(world: &WorldDir, town: &TownName, after: u64) -> io::Result<Vec<(u64, Entry)>> {
    if let Some(entries) = carry_source::entries(world, town)? {
        return Ok(entries.into_iter().filter(|(seq, _)| *seq > after).collect());
    }
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
    let taken = kept_files(directory)?.last().map(|(seq, _, _)| *seq).unwrap_or(0);
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
