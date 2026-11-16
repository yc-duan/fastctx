//! A project's ledger as this machine keeps it from the hub's projection
//! for the account it reads the project as (design §28.58, §29 2.4): the
//! hub writes each entry, as that account may know it, into the account's
//! records as `ledger/<project>/<machines>`, and this machine takes them
//! from its copy of those records — no ledger is replicated beside them.
//! Each is kept under `projected/` by its number and written out into the
//! project's files. A project this machine kept as signed entries before is
//! made again from the projection beside them, then put in their place. A
//! hub that writes no projection, or records not pulled here yet, leave the
//! project replicated as before (`ledger::take`).

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fastctx_content::v2::Library;
use fastctx_net::authorization::Action;
use fastctx_net::ledger::Projection;
use fastctx_net::link::Link;
use fastctx_net::sign::{Domain, SecretKey, Signed};
use fastctx_platform::ensure_private_dir;
use fastctx_types::TownName;
use fastctx_types::access::{Acting, TownId};
use fastctx_types::layout::TownDir;

use crate::Node;

/// Where a project's projection is read from here: the account's records'
/// scope and the stream in it.
pub(crate) struct Source {
    scope: String,
    stream: String,
}

impl Source {
    /// What a project kept from it is kept as: another account, or the
    /// machines it may work on changed, tell its entries otherwise under
    /// the same numbers, so the project is made again from the first.
    fn named(&self) -> String {
        format!("{} {}", self.scope, self.stream)
    }
}

/// Where what a project is kept from is written down, beside it.
pub(crate) const FROM: &str = "projected-from.json";

/// What `directory`'s project is kept from now: none for one replicated,
/// or not kept yet. Caches of what its entries say are of what it is kept
/// from, as of the ledger they were read from.
pub(crate) fn kept_from(directory: &TownDir) -> io::Result<Option<String>> {
    fastctx_net::link::state::read_or(&directory.path().join(FROM), None)
}

/// How long a take waits for the projection to come up to the latest entry.
const WAITS: Duration = Duration::from_secs(10);

/// The projection of `town`'s ledger for the first account logged in here,
/// by name, that reads the project, where the copy here holds that
/// account's records pulled whole; none otherwise.
///
/// The stream is the one of the machines that account may work on as the
/// directory here tells them; a directory cut for this machine may tell
/// fewer than the hub's, so where the records here hold no such stream but
/// one other of the project, that one is it — the hub has the account hold
/// the stream of its machines now alone.
pub(crate) fn source(node: &Node, link: &Link, town: &TownId) -> Option<Source> {
    let library = &node.assets.as_ref()?.library;
    let view = link.view();
    let hub = link.membership().id.to_string();
    let mut accounts = node.accounts_on(link, &Acting::Here);
    accounts.sort();
    let account =
        accounts.into_iter().find(|account| view.directory.authorize(account, Action::Project(town)).is_ok())?;
    let opened = view.directory.account(&account)?.opened;
    let scope = fastctx_types::assets::records_key(&hub, &account, opened.as_millisecond());
    let held = library.held_scopes().ok()?.into_iter().find(|held| held.key == scope)?;
    if held.pulled != Some(held.holding) {
        return None;
    }
    let usable = fastctx_net::ledger::reader_machines(&view.directory, &account, None);
    let named = fastctx_net::ledger::reader_stream(town, &fastctx_net::ledger::reader_digest(&usable));
    let prefix = format!("ledger/{town}/");
    let streams: Vec<String> = library
        .streams(&scope)
        .ok()?
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with(&prefix))
        .collect();
    let stream = match streams.contains(&named) {
        true => named,
        false => match streams.as_slice() {
            [only] => only.clone(),
            _ => return None,
        },
    };
    Some(Source { scope, stream })
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

/// The number of the last entry of the projection here.
fn last(library: &Library, source: &Source) -> Result<u64, String> {
    let mut at = 0;
    loop {
        let page = entries(library, source, at, 256)?;
        match page.last() {
            Some(projection) => at = projection.seq,
            None => return Ok(at),
        }
    }
}

/// Takes `town`'s entries through `latest` from its projection, kept as
/// `name`, waiting a while for the projection to come up to it. Says
/// whether anything changed.
pub(crate) async fn take(
    node: &Arc<Node>,
    link: &Arc<Link>,
    name: &TownName,
    town: &TownId,
    latest: u64,
    source: Source,
) -> Result<bool, String> {
    let library = node.assets.as_ref().ok_or("the asset library is not open")?.library.clone();
    let until = Instant::now() + WAITS;
    loop {
        let (reading, looked) =
            (library.clone(), Source { scope: source.scope.clone(), stream: source.stream.clone() });
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
        tokio::time::sleep(Duration::from_millis(200)).await;
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
            again(&library, &source, &directory, &key)
                .map_err(|error| format!("making {name}'s ledger from its projection: {error}"))?;
            return Ok(true);
        }
        let mut after = super::cursor(&directory, &Default::default()).map_err(|error| error.to_string())?;
        let mut changed = false;
        loop {
            let page = entries(&library, &source, after, 256)?;
            let Some(last) = page.last().map(|projection| projection.seq) else { break };
            for projection in &page {
                keep(&directory.projected(), &directory.ledger(), projection, &key)
                    .map_err(|error| format!("keeping entry {} of {name}: {error}", projection.seq))?;
            }
            after = last;
            changed = true;
        }
        Ok::<_, String>(changed)
    })
    .await
}

/// Keeps `projection` under `projected`, under this machine's word, and
/// writes its files under `ledger`.
fn keep(
    projected: &std::path::Path,
    ledger: &std::path::Path,
    projection: &Projection,
    key: &SecretKey,
) -> io::Result<()> {
    ensure_private_dir(projected)?;
    let bytes = serde_json::to_vec(&Signed::new(key, Domain::LedgerKept, projection)).map_err(io::Error::other)?;
    super::same_or_new(&projected.join(format!("{:020}.json", projection.seq)), &bytes)?;
    super::write_files(ledger, &projection.files)
}

/// Makes a project from its projection: every entry kept and written out
/// beside what is kept of it now, then put in its place, with what it is
/// kept from; the signed entries of before and what their reading kept let
/// go of. A stop on the way makes it again from the start.
fn again(library: &Library, source: &Source, directory: &TownDir, key: &SecretKey) -> io::Result<()> {
    let root = directory.path();
    let (projected, ledger) = (root.join("projected.next"), root.join("ledger.next"));
    super::remove(&projected)?;
    super::remove(&ledger)?;
    let mut after = 0;
    loop {
        let page = entries(library, source, after, 256).map_err(io::Error::other)?;
        let Some(last) = page.last().map(|projection| projection.seq) else { break };
        for projection in &page {
            keep(&projected, &ledger, projection, key)?;
        }
        after = last;
    }
    ensure_private_dir(&ledger)?;
    ensure_private_dir(&projected)?;
    // What it was kept from goes first: a stop past here leaves it naming
    // nothing, and the project is made again.
    forget(&root.join(FROM))?;
    super::remove(&directory.ledger())?;
    fastctx_platform::durable_file::rename_durably(&ledger, &directory.ledger())?;
    super::remove(&directory.projected())?;
    fastctx_platform::durable_file::rename_durably(&projected, &directory.projected())?;
    super::remove(&directory.signed())?;
    for kept in ["projection-through.json", "refused.json"] {
        forget(&root.join(kept))?;
    }
    fastctx_net::link::state::write(&root.join(FROM), &source.named())
}

/// Removes the file at `path`, if there is one.
fn forget(path: &std::path::Path) -> io::Result<()> {
    match fastctx_platform::disk::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
