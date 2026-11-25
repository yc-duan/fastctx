//! A project's ledger as this machine keeps it from the hub's projection
//! for the account it reads the project as (design §28.58, §29 2.4): the
//! hub writes each entry, as that account may know it, into the account's
//! records as `ledger/<project>/<machines>`, and this machine takes them
//! from its copy of those records — no ledger is replicated beside them.
//! Each is kept under `projected/` by its number, under this machine's own
//! word, and written out into the project's files. A project this machine
//! kept as signed entries before is made again from the projection beside
//! them, then put in their place. A hub of a release before projections
//! leaves a project replicated as before (`ledger::take`); one that writes
//! them is waited for, never replicated from beside them: replicated, an
//! entry tells what the account may not know (codex-357de52a, third round
//! #1).

use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastctx_content::v2::Library;
use fastctx_net::authorization::Action;
use fastctx_net::ledger::Projection;
use fastctx_net::link::Link;
use fastctx_net::sign::{Domain, SecretKey, Signed};
use fastctx_platform::ensure_private_dir;
use fastctx_types::access::{Acting, TownId};
use fastctx_types::layout::TownDir;
use fastctx_types::{MachineName, TownName};

use crate::Node;

/// Where a project's projection is read from here: the account's records'
/// scope and the stream in it, and the machines the account may work on,
/// which the stream is of.
#[derive(Clone)]
pub(crate) struct Source {
    scope: String,
    stream: String,
    machines: BTreeSet<MachineName>,
}

impl Source {
    /// What a project kept from it is kept as: another account, or the
    /// machines it may work on changed, tell its entries otherwise under
    /// the same numbers, so the project is made again from the first. A
    /// project kept before its entries were kept as `KeptEntry`, named
    /// without `kept`, is made again too (codex-357de52a, third round #7).
    fn named(&self) -> String {
        format!("{} {} kept", self.scope, self.stream)
    }
}

/// Where what a project is kept from is written down, beside it.
pub(crate) const FROM: &str = "projected-from.json";

/// Where the scope and the machines a project is kept for are written down,
/// beside it: kept for machines the account may still all work on, it may
/// be read on while the projection for more comes (third round #6).
pub(crate) const FOR: &str = "projected-for.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct KeptFor {
    scope: String,
    machines: BTreeSet<MachineName>,
}

/// Where a project is made again before it is put in place.
pub(crate) const NEXT: (&str, &str) = ("projected.next", "ledger.next");

/// What `directory`'s project is kept from now: none for one replicated,
/// or not kept yet. Caches of what its entries say are of what it is kept
/// from, as of the ledger they were read from.
pub(crate) fn kept_from(directory: &TownDir) -> io::Result<Option<String>> {
    fastctx_net::link::state::read_or(&directory.path().join(FROM), None)
}

/// How long a take waits for the projection to be here and to come up to
/// the latest entry.
pub(crate) const WAITS: Duration = Duration::from_secs(10);

/// How often it is looked at meanwhile.
pub(crate) const LOOKS: Duration = Duration::from_millis(200);

/// Entries of a projection read at a time: each may be as large as an
/// entry may be, so a few (codex-357de52a #4).
const AT_ONCE: usize = 8;

/// What this machine reads `town`'s ledger from.
pub(crate) enum Seen {
    /// The projection for the first account logged in here, by name, that
    /// reads the project, in its records pulled here whole: the stream of
    /// the machines that account may work on now, as the directory here
    /// tells them — what the hub writes too (`for_reader::usable`).
    At(Source),
    /// The hub writes projections and that stream is not here yet: the
    /// account's records not here whole yet, or the asset library not open,
    /// or the machines it may work on changed, or the hub has not written
    /// one for it yet. Never another stream of the project in its place, nor
    /// entries replicated beside: they tell what this account may not know
    /// (codex-357de52a #2, third round #1, fourth round #4). Named are the
    /// scope and machines it is waited for.
    Waiting { scope: String, machines: BTreeSet<MachineName> },
    /// No account here reads the project, or the hub is of a release before
    /// projections.
    Not,
    /// This machine is shared whole on the hub, a share waiting among them:
    /// it holds as a copy only what every one of its holders reads, and an
    /// account's records, the projections in them, are its account's alone
    /// (the hub's `holders_read_known`), so none ever comes here. Its
    /// projects are taken as entries replicated, as before projections and
    /// as the hub's own rule for who keeps a project allows: what was kept
    /// of a projection from before it was shared goes first.
    Whole,
}

pub(crate) fn source(node: &Node, link: &Link, town: &TownId) -> Seen {
    let view = link.view();
    let hub = link.membership().id.to_string();
    let me = view.directory.canonical(&link.name()).clone();
    if view.asset_sync > 0 && view.directory.shares.iter().any(|share| share.machine == me) {
        return Seen::Whole;
    }
    let mut accounts = node.accounts_on(link, &Acting::Here);
    accounts.sort();
    let Some(account) =
        accounts.into_iter().find(|account| view.directory.authorize(account, Action::Project(town)).is_ok())
    else {
        return Seen::Not;
    };
    let Some(opened) = view.directory.account(&account).map(|found| found.opened) else { return Seen::Not };
    let scope = fastctx_types::assets::records_key(&hub, &account, opened.as_millisecond());
    let machines = fastctx_net::ledger::reader_machines(&view.directory, &account, None);
    // A hub that speaks the asset sync writes projections: what is not here
    // of one is waited for.
    let otherwise = |scope, machines| if view.asset_sync > 0 { Seen::Waiting { scope, machines } } else { Seen::Not };
    let Some(assets) = node.assets.as_ref() else { return otherwise(scope, machines) };
    let library = &assets.library;
    let Some(held) = library.held_scopes().ok().and_then(|held| held.into_iter().find(|held| held.key == scope)) else {
        return otherwise(scope, machines);
    };
    if held.pulled != Some(held.holding) {
        return otherwise(scope, machines);
    }
    let stream = fastctx_net::ledger::reader_stream(town, &fastctx_net::ledger::reader_digest(&machines));
    match library.streams(&scope) {
        Ok(streams) if streams.iter().any(|(name, _)| *name == stream) => Seen::At(Source { scope, stream, machines }),
        Ok(_) => Seen::Waiting { scope, machines },
        Err(_) => otherwise(scope, machines),
    }
}

/// `still_readable`, for what the project is to be taken from now.
pub(crate) fn readable_for(node: &Node, name: &TownName, source: &Source) -> bool {
    still_readable(node, name, &source.scope, &source.machines)
}

/// Whether what is kept of `name` was kept for `scope` and machines all
/// among `machines`: it tells nothing the account may not know now, and is
/// read on while the projection for them comes. One that does not say what
/// it was kept for is not.
pub(crate) fn still_readable(node: &Node, name: &TownName, scope: &str, machines: &BTreeSet<MachineName>) -> bool {
    let path = node.world.town(name).path().join(FOR);
    fastctx_net::link::state::read_or::<Option<KeptFor>>(&path, None)
        .ok()
        .flatten()
        .is_some_and(|kept| kept.scope == scope && kept.machines.is_subset(machines))
}

/// What this machine keeps of a projected entry, under its own word: the
/// project's too, so one entry cannot pass for another project's
/// (codex-357de52a #7).
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct KeptEntry {
    pub town: TownId,
    pub projection: Projection,
}

/// The entries of the projection after `after`, at most `most`, each
/// checked against what its record says of it.
fn entries(library: &Library, source: &Source, after: u64, most: usize) -> Result<Vec<Projection>, String> {
    let from = after.to_be_bytes().to_vec();
    let page = library
        .page(&source.scope, &source.stream, Some((from.clone(), from)), most)
        .map_err(|error| error.to_string())?;
    let mut found = Vec::with_capacity(page.len());
    for entry in page {
        let said: serde_json::Value = serde_json::from_slice(&entry.body).map_err(|error| error.to_string())?;
        let [object] = entry.objects.as_slice() else { return Err("a projected entry is its one object".into()) };
        let bytes =
            crate::timeline::copy::object_bytes(library, object).ok_or("a projected entry is not all here yet")?;
        let digest: String = fastctx_net::sign::sha256(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        if said.get("sha256").and_then(serde_json::Value::as_str) != Some(digest.as_str()) {
            return Err("a projected entry is not what its record says".into());
        }
        let projection: Projection = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if said.get("seq").and_then(serde_json::Value::as_u64) != Some(projection.seq) {
            return Err("a projected entry is not the number its record says".into());
        }
        found.push(projection);
    }
    Ok(found)
}

/// The number of the last entry of the projection here, as its record
/// says: the stream is ordered by number (codex-357de52a #8).
fn last(library: &Library, source: &Source) -> Result<u64, String> {
    let page = library.page_before(&source.scope, &source.stream, None, 1).map_err(|error| error.to_string())?;
    let Some(entry) = page.first() else { return Ok(0) };
    let said: serde_json::Value = serde_json::from_slice(&entry.body).map_err(|error| error.to_string())?;
    said.get("seq").and_then(serde_json::Value::as_u64).ok_or_else(|| "a projected entry says no number".into())
}

/// Lets go of what this machine kept of `name` from a projection it may no
/// longer read: its entries, files and what they were kept from, and any
/// making of it again left half done. Call with the ledgers' sync held.
pub(crate) fn clear(node: &Node, name: &TownName) -> io::Result<()> {
    let directory = node.world.town(name);
    let _map_changed = node.topics.writing(format!("{}{}", fastctx_types::topics::MAP_PREFIX, name));
    forget(&directory.path().join(FROM))?;
    forget(&directory.path().join(FOR))?;
    for folder in
        [directory.projected(), directory.ledger(), directory.path().join(NEXT.0), directory.path().join(NEXT.1)]
    {
        super::remove(&folder)?;
    }
    Ok(())
}

/// Takes `town`'s entries through `latest` from its projection, kept as
/// `name`, waiting until `until` for the projection to come up to it. Says
/// whether anything changed.
pub(crate) async fn take(
    node: &Arc<Node>,
    link: &Arc<Link>,
    name: &TownName,
    town: &TownId,
    latest: u64,
    source: Source,
    until: Instant,
) -> Result<bool, String> {
    let library = node.assets.as_ref().ok_or("the asset library is not open")?.library.clone();
    loop {
        let (reading, looked) = (library.clone(), source.clone());
        let here = fastctx_platform::task::spawn_blocking(move || last(&reading, &looked))
            .await
            .map_err(|error| error.to_string())??;
        if here >= latest {
            break;
        }
        if Instant::now() >= until {
            return Err(format!(
                "the hub's projection of {name}'s ledger has come up to {here} of {latest} here so far"
            ));
        }
        tokio::time::sleep(LOOKS).await;
    }
    let _syncing = node.syncing.lock().await;
    let (keeping, link, name, town) = (node.clone(), link.clone(), name.clone(), town.clone());
    fastctx_platform::task::blocking(move || {
        super::check_place(&keeping, &link, &name, &town)?;
        let directory = keeping.world.town(&name);
        let _map_changed = keeping.topics.writing(format!("{}{}", fastctx_types::topics::MAP_PREFIX, name));
        let key = super::own_key(&keeping.world).map_err(|error| format!("this machine's key: {error}"))?;
        // Kept as signed entries, from another projection, or not at all
        // yet: made from the first.
        let from = kept_from(&directory).map_err(|error| error.to_string())?;
        if fastctx_platform::disk::is_dir(directory.signed())
            || !fastctx_platform::disk::is_dir(directory.projected())
            || from.as_deref() != Some(source.named().as_str())
        {
            again(&library, &source, &directory, &town, &key)
                .map_err(|error| format!("making {name}'s ledger from its projection: {error}"))?;
            return Ok(true);
        }
        let mut after = super::cursor(&directory, &Default::default()).map_err(|error| error.to_string())?;
        let mut changed = false;
        loop {
            let page = entries(&library, &source, after, AT_ONCE)?;
            let Some(last) = page.last().map(|projection| projection.seq) else { break };
            for projection in &page {
                keep(&directory.projected(), &directory.ledger(), &town, projection, &key)
                    .map_err(|error| format!("keeping entry {} of {name}: {error}", projection.seq))?;
            }
            after = last;
            changed = true;
        }
        Ok::<_, String>(changed)
    })
    .await
}

/// Keeps `projection` of `town` under `projected`, under this machine's
/// word, and writes its files under `ledger`.
fn keep(
    projected: &std::path::Path,
    ledger: &std::path::Path,
    town: &TownId,
    projection: &Projection,
    key: &SecretKey,
) -> io::Result<()> {
    ensure_private_dir(projected)?;
    let kept = KeptEntry { town: town.clone(), projection: projection.clone() };
    let bytes = serde_json::to_vec(&Signed::new(key, Domain::LedgerKept, &kept)).map_err(io::Error::other)?;
    super::same_or_new(&projected.join(format!("{:020}.json", projection.seq)), &bytes)?;
    super::write_files(ledger, &projection.files)
}

/// Makes a project from its projection: every entry kept and written out
/// beside what is kept of it now, then put in its place, with what it is
/// kept from; the signed entries of before and what their reading kept let
/// go of. A stop on the way makes it again from the start.
fn again(library: &Library, source: &Source, directory: &TownDir, town: &TownId, key: &SecretKey) -> io::Result<()> {
    let root = directory.path();
    let (projected, ledger) = (root.join(NEXT.0), root.join(NEXT.1));
    super::remove(&projected)?;
    super::remove(&ledger)?;
    let mut after = 0;
    loop {
        let page = entries(library, source, after, AT_ONCE).map_err(io::Error::other)?;
        let Some(last) = page.last().map(|projection| projection.seq) else { break };
        for projection in &page {
            keep(&projected, &ledger, town, projection, key)?;
        }
        after = last;
    }
    ensure_private_dir(&ledger)?;
    ensure_private_dir(&projected)?;
    // What it was kept from goes first: a stop past here leaves it naming
    // nothing, and the project is made again.
    forget(&root.join(FROM))?;
    forget(&root.join(FOR))?;
    super::remove(&directory.ledger())?;
    fastctx_platform::durable_file::rename_durably(&ledger, &directory.ledger())?;
    super::remove(&directory.projected())?;
    fastctx_platform::durable_file::rename_durably(&projected, &directory.projected())?;
    super::remove(&directory.signed())?;
    for kept in ["projection-through.json", "refused.json"] {
        forget(&root.join(kept))?;
    }
    fastctx_net::link::state::write(
        &root.join(FOR),
        &Some(KeptFor { scope: source.scope.clone(), machines: source.machines.clone() }),
    )?;
    fastctx_net::link::state::write(&root.join(FROM), &source.named())
}

/// Removes the file at `path`, if there is one.
fn forget(path: &std::path::Path) -> io::Result<()> {
    match fastctx_platform::disk::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
