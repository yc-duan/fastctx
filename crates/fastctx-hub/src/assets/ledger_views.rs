//! Each project's ledger as each of its readers may know it, written into
//! that reader's own records (§29, 2.4): a stream the hub orders, an entry
//! by its number, its projection the one object. No ledger goes whole into
//! a project's scope any more: an entry names machines not every reader of
//! the project may know of.
//!
//! The stream is named by the project and by the machines the projection
//! was made by, as a digest: `ledger/<project>/<digest>`. A reader holds
//! the stream of the machines it may work on now, and of a project it reads
//! now, alone (`holds`): one whose machines changed holds the new stream
//! and lets the old go, its frames and objects with it; one that no longer
//! reads the project holds none. So what was told under rights since taken
//! away is never told again, on a new device either.
//!
//! A look, after each entry the hub takes (that project's alone) and every
//! minute (all), writes into each reader's stream the entries it lacks; a
//! stream made anew is written from the first, an entry already there as
//! it is not written again.

use super::{Assets, scopes, told};
use fastctx_content::v2::{Kind as _, KindRecord, LEASE, ProjectLedger};
use fastctx_content::{Record as Item, State};
use fastctx_net::directory::Directory;
use fastctx_types::access::TownId;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The projections the hub keeps, in its control database.
pub(crate) const SCHEMA: &str = "
-- Each project's ledger as each reader account reads it (§29, 2.4): the
-- account as opened (in milliseconds), the project, the digest of the
-- machines the projection was made by and the last entry written.
CREATE TABLE IF NOT EXISTS asset_ledger_views (
    account TEXT NOT NULL, opened INTEGER NOT NULL, town TEXT NOT NULL, machines BLOB NOT NULL,
    through INTEGER NOT NULL, PRIMARY KEY (account, opened, town)
) WITHOUT ROWID;
";

/// The digest's hex digits a stream is named by.
const NAMED_BY: usize = 16;

/// The stream of `town`'s ledger in a reader's records, for the machines
/// whose digest is `machines`.
pub(crate) fn stream(town: &TownId, machines: &[u8]) -> String {
    let named = fastctx_net::ledger::reader_stream(town, machines);
    debug_assert_eq!(named.rsplit('/').next().map(str::len), Some(NAMED_BY));
    debug_assert!(named.starts_with(ProjectLedger::STREAM));
    named
}

/// Whether `account` holds the stream `name` (after `ledger/`) of its
/// records: of a project it reads now, made by the machines it may work on
/// now (§29, 2.4).
pub(crate) fn holds(directory: &Directory, account: &AccountName, name: &str) -> bool {
    let Some((town, _)) = name.split_once('/') else { return false };
    let Ok(town) = TownId::try_from(town.to_string()) else { return false };
    if directory.authorize(account, fastctx_net::authorization::Action::Project(&town)).is_err() {
        return false;
    }
    let usable = crate::ledger::for_reader::usable(directory, account, None);
    stream(&town, &crate::ledger::for_reader::digest(&usable)) == format!("{}/{name}", ProjectLedger::STREAM)
}

/// Entries written in one frame at most.
const AT_ONCE: usize = 64;

/// One look at a time; what is asked for meanwhile is looked at after it:
/// some projects, or all (`None`).
static LOOKING: Mutex<()> = Mutex::new(());
static AGAIN: AtomicBool = AtomicBool::new(false);
static ASKED: Mutex<Option<BTreeSet<TownId>>> = Mutex::new(Some(BTreeSet::new()));

/// Looks again at every project, off the caller's thread.
pub(crate) fn look(hub: &Arc<crate::Hub>) {
    ask(hub, None);
}

/// Looks again at `town`, which took an entry, off the caller's thread.
pub(crate) fn look_at(hub: &Arc<crate::Hub>, town: &TownId) {
    ask(hub, Some(town.clone()));
}

fn ask(hub: &Arc<crate::Hub>, town: Option<TownId>) {
    {
        let mut asked = ASKED.lock().expect("asked");
        match (asked.as_mut(), town) {
            (Some(towns), Some(town)) => {
                towns.insert(town);
            }
            _ => *asked = None,
        }
    }
    let hub = hub.clone();
    fastctx_platform::task::spawn_blocking(move || {
        AGAIN.store(true, Ordering::SeqCst);
        loop {
            let Ok(looking) = LOOKING.try_lock() else { return };
            while AGAIN.swap(false, Ordering::SeqCst) {
                let only = ASKED.lock().expect("asked").replace(BTreeSet::new());
                if let Err(error) = look_now(&hub, only.as_ref()) {
                    hub.log.line(format_args!("writing the projects' ledgers into their readers' records: {error}"));
                }
            }
            drop(looking);
            // One that asked between the last look and the letting go found
            // a look going on and left: it is looked at here still.
            if !AGAIN.load(Ordering::SeqCst) {
                return;
            }
        }
    });
}

/// A reader of a project: the account as opened, the machines it may work
/// on and their digest.
struct Reader {
    account: AccountName,
    opened: Timestamp,
    usable: Arc<BTreeSet<MachineName>>,
    digest: Vec<u8>,
}

type Kept = BTreeMap<(String, i64, String), (Vec<u8>, u64)>;

/// Looks at the projects `only` names, or at all: each reader's stream gets
/// what it lacks. One reader's failure holds up no other's; the first is
/// told.
fn look_now(hub: &crate::Hub, only: Option<&BTreeSet<TownId>>) -> Result<(), String> {
    let assets = &hub.assets;
    let towns: BTreeMap<TownId, u64> = hub
        .ledgers
        .lock()
        .expect("never poisoned")
        .towns
        .iter()
        .filter(|(town, _)| only.is_none_or(|only| only.contains(*town)))
        .map(|(town, ledger)| (town.clone(), ledger.next))
        .collect();
    let readers: BTreeMap<TownId, Vec<Reader>> = {
        let store = hub.store();
        let directory = &store.book.directory;
        // The machines each reader may work on, once a look whatever it
        // reads: only accounts that read one of these projects are asked.
        let mut usable: BTreeMap<AccountName, (Arc<BTreeSet<MachineName>>, Vec<u8>)> = BTreeMap::new();
        let mut readers = BTreeMap::new();
        for town in towns.keys() {
            let mut of = Vec::new();
            for account in &directory.accounts {
                if directory.authorize(&account.name, fastctx_net::authorization::Action::Project(town)).is_err() {
                    continue;
                }
                let (machines, digest) = usable
                    .entry(account.name.clone())
                    .or_insert_with(|| {
                        let machines = crate::ledger::for_reader::usable(directory, &account.name, None);
                        let digest = crate::ledger::for_reader::digest(&machines);
                        (Arc::new(machines), digest)
                    })
                    .clone();
                of.push(Reader { account: account.name.clone(), opened: account.opened, usable: machines, digest });
            }
            readers.insert(town.clone(), of);
        }
        readers
    };
    let mut kept = kept(assets)?;
    let hub_id = hub.store().hub.id.clone();
    let mut failed: Option<String> = None;
    for (town, next) in &towns {
        for reader in readers.get(town).into_iter().flatten() {
            let key = (reader.account.to_string(), reader.opened.as_millisecond(), town.to_string());
            let row = kept.remove(&key);
            if let Err(error) = project_for(hub, &hub_id, town, *next, reader, row, &key) {
                failed.get_or_insert(format!("{} of {town}: {error}", reader.account));
            }
        }
    }
    // What is kept and read no more: its reader no longer reads the project,
    // or the project is gone. Its stream is held by none (`holds`); what is
    // kept of it here goes.
    for (key, _) in kept.into_iter().filter(|((_, _, town), _)| {
        only.is_none_or(|only| TownId::try_from(town.clone()).is_ok_and(|town| only.contains(&town)))
    }) {
        if let Err(error) = forget(assets, &key) {
            failed.get_or_insert(error);
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Writes into `reader`'s stream of `town` the entries up to `next` it
/// lacks.
fn project_for(
    hub: &crate::Hub,
    hub_id: &fastctx_net::ids::HubId,
    town: &TownId,
    next: u64,
    reader: &Reader,
    row: Option<(Vec<u8>, u64)>,
    key: &(String, i64, String),
) -> Result<(), String> {
    let assets = &hub.assets;
    let from = match &row {
        Some((machines, through)) if *machines == reader.digest => through + 1,
        // Its machines changed: a stream of their own, from the first.
        _ => 1,
    };
    if from >= next && row.is_some() {
        return Ok(());
    }
    let Some((scope, library)) = told::scope_of(assets, &reader.account, reader.opened)? else { return Ok(()) };
    let name = stream(town, &reader.digest);
    let usable = |machine: &MachineName| reader.usable.contains(machine);
    // A history revision's author as the first version tells it to one who
    // may work only on those machines; the projection depends on them alone.
    let history = |revision: &mut fastctx_types::history::HistoryRevision| revision.author.stamp.for_reader(&usable);
    let mut records = Vec::new();
    for seq in from..next {
        let at = hub.ledgers.lock().expect("never poisoned").entry_at(town, seq);
        let signed = crate::ledger::read_entry(&at).map_err(|error| format!("entry {seq}: {error}"))?;
        let entry = fastctx_net::ledger::open(&signed, hub_id, town)?;
        let projection = crate::ledger::for_reader::entry(seq, &entry, &usable, &history);
        if let Some(record) = record(&scope, &library, &name, &projection)? {
            records.push(record);
        }
        if records.len() == AT_ONCE {
            write(assets, &scope, &library, &name, std::mem::take(&mut records))?;
        }
    }
    write(assets, &scope, &library, &name, records)?;
    keep(assets, key, &reader.digest, next.saturating_sub(1))
}

/// The record of `projection`, when the library does not hold it as it is.
fn record(
    scope: &scopes::Scope,
    library: &fastctx_content::v2::Library,
    stream: &str,
    projection: &fastctx_net::ledger::Projection,
) -> Result<Option<KindRecord>, String> {
    let bytes = serde_json::to_vec(projection).map_err(|error| error.to_string())?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let body = serde_json::to_vec(&serde_json::json!({ "seq": projection.seq, "size": bytes.len(), "sha256": digest }))
        .map_err(|error| error.to_string())?;
    let item = projection.seq.to_be_bytes().to_vec();
    let error = |error: fastctx_content::Error| error.to_string();
    if library.item(&scope.descriptor.scope, stream, &item).map_err(error)?.is_some_and(|held| held.body == body) {
        return Ok(None);
    }
    let lease = library.put(&mut &bytes[..], "ledger", LEASE).map_err(error)?;
    Ok(Some(KindRecord {
        kind: ProjectLedger::KIND,
        schema: ProjectLedger::SCHEMA,
        record: Item { item: item.clone(), order: item, state: State::Is { body, objects: vec![lease.object] } },
    }))
}

/// Writes `records` into `stream` of the reader's records.
fn write(
    assets: &Assets,
    scope: &scopes::Scope,
    library: &fastctx_content::v2::Library,
    stream: &str,
    records: Vec<KindRecord>,
) -> Result<(), String> {
    let _writing = super::mirror::writing(&scope.descriptor.scope);
    super::mirror::apply(assets, scope, library, stream, records)
}

fn kept(assets: &Assets) -> Result<Kept, String> {
    assets
        .control
        .read_now(|c| {
            let mut statement =
                c.prepare_cached("SELECT account, opened, town, machines, through FROM asset_ledger_views")?;
            let rows = statement.query_map([], |row| {
                Ok(((row.get(0)?, row.get(1)?, row.get(2)?), (row.get(3)?, row.get::<_, i64>(4)? as u64)))
            })?;
            Ok(rows.collect::<std::result::Result<Kept, _>>()?)
        })
        .map_err(|error| error.to_string())
}

fn keep(assets: &Assets, key: &(String, i64, String), machines: &[u8], through: u64) -> Result<(), String> {
    let (key, machines) = (key.clone(), machines.to_vec());
    assets
        .control
        .write_now(128 + machines.len(), move |c| {
            c.prepare_cached(
                "INSERT INTO asset_ledger_views (account, opened, town, machines, through) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (account, opened, town) DO UPDATE SET machines = excluded.machines, through = excluded.through",
            )?
            .execute(fastctx_storage::rusqlite::params![key.0, key.1, key.2, machines, through as i64])?;
            Ok(())
        })
        .map_err(|error| error.to_string())
}

fn forget(assets: &Assets, key: &(String, i64, String)) -> Result<(), String> {
    let key = key.clone();
    assets
        .control
        .write_now(128, move |c| {
            c.prepare_cached("DELETE FROM asset_ledger_views WHERE account = ?1 AND opened = ?2 AND town = ?3")?
                .execute(fastctx_storage::rusqlite::params![key.0, key.1, key.2])?;
            Ok(())
        })
        .map_err(|error| error.to_string())
}
