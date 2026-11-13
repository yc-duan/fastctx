//! Each project's ledger as each of its readers may know it, written into
//! that reader's own records (§29, 2.4): a stream `ledger/<project>` the hub
//! orders, an entry by its number, its projection the one object. No ledger
//! goes whole into a project's scope any more: an entry names machines not
//! every reader of the project may know of.
//!
//! Each projection keeps the machines it was made by. A look, after each
//! entry the hub takes and every minute, writes the entries a projection
//! lacks, makes a projection again when its machines changed — writing only
//! the entries that come out otherwise — and takes it away from an account
//! that no longer reads the project.

use super::{Assets, scopes, told};
use fastctx_content::v2::{Kind as _, KindRecord, LEASE, ProjectLedger};
use fastctx_content::{Record as Item, State};
use fastctx_types::AccountName;
use fastctx_types::access::TownId;
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

/// The stream of `town`'s ledger in a reader's records.
pub(crate) fn stream(town: &TownId) -> String {
    format!("{}/{town}", ProjectLedger::STREAM)
}

/// Entries written in one frame at most.
const AT_ONCE: usize = 64;

/// One look at a time; one asked for meanwhile is made after it.
static LOOKING: Mutex<()> = Mutex::new(());
static AGAIN: AtomicBool = AtomicBool::new(false);

/// Looks again, off the caller's thread.
pub(crate) fn look(hub: &Arc<crate::Hub>) {
    let hub = hub.clone();
    fastctx_platform::task::spawn_blocking(move || {
        AGAIN.store(true, Ordering::SeqCst);
        let Ok(_looking) = LOOKING.try_lock() else { return };
        while AGAIN.swap(false, Ordering::SeqCst) {
            if let Err(error) = look_now(&hub) {
                hub.log.line(format_args!("writing the projects' ledgers into their readers' records: {error}"));
            }
        }
    });
}

/// A reader of a project: the account as opened, the machines it may work
/// on and their digest.
struct Reader {
    account: AccountName,
    opened: Timestamp,
    usable: BTreeSet<fastctx_types::MachineName>,
    digest: Vec<u8>,
}

type Kept = BTreeMap<(String, i64, String), (Vec<u8>, u64)>;

/// The accounts in the directory, by name and when opened in milliseconds.
type Present = BTreeMap<(String, i64), Timestamp>;

fn look_now(hub: &crate::Hub) -> Result<(), String> {
    let assets = &hub.assets;
    let towns: BTreeMap<TownId, u64> =
        hub.ledgers.lock().expect("never poisoned").towns.iter().map(|(town, ledger)| (town.clone(), ledger.next)).collect();
    let (readers, accounts): (BTreeMap<TownId, Vec<Reader>>, Present) = {
        let store = hub.store();
        let directory = &store.book.directory;
        let accounts: Vec<_> = directory
            .accounts
            .iter()
            .map(|account| {
                let usable = crate::ledger::for_reader::usable(directory, &account.name, None);
                let digest = crate::ledger::for_reader::digest(&usable);
                (account, usable, digest)
            })
            .collect();
        let readers = towns
            .keys()
            .map(|town| {
                let of = accounts
                    .iter()
                    .filter(|(account, ..)| {
                        directory.authorize(&account.name, fastctx_net::authorization::Action::Project(town)).is_ok()
                    })
                    .map(|(account, usable, digest)| Reader {
                        account: account.name.clone(),
                        opened: account.opened,
                        usable: usable.clone(),
                        digest: digest.clone(),
                    })
                    .collect();
                (town.clone(), of)
            })
            .collect();
        let present = directory
            .accounts
            .iter()
            .map(|account| ((account.name.to_string(), account.opened.as_millisecond()), account.opened))
            .collect();
        (readers, present)
    };
    let mut kept = kept(assets)?;
    for (town, next) in &towns {
        for reader in readers.get(town).into_iter().flatten() {
            let key = (reader.account.to_string(), reader.opened.as_millisecond(), town.to_string());
            let row = kept.remove(&key);
            let from = match &row {
                Some((machines, through)) if *machines == reader.digest => through + 1,
                _ => 1,
            };
            if from >= *next && row.is_some() {
                continue;
            }
            let Some((scope, library)) = told::scope_of(assets, &reader.account, reader.opened)? else { continue };
            let usable = |machine: &fastctx_types::MachineName| reader.usable.contains(machine);
            let history = |revision: &mut fastctx_types::history::HistoryRevision| {
                crate::history_projection::revision(hub, &hub.store().book.directory, &reader.account, revision)
            };
            let hub_id = hub.store().hub.id.clone();
            let mut records = Vec::new();
            for seq in from..*next {
                let path = hub.ledgers.lock().expect("never poisoned").entry_path(town, seq);
                let signed = crate::ledger::read_entry(&path).map_err(|error| format!("entry {seq} of {town}: {error}"))?;
                let entry = fastctx_net::ledger::open(&signed, &hub_id, town)?;
                let projection = crate::ledger::for_reader::entry(seq, &entry, &usable, &history);
                if let Some(record) = record(&scope, &library, town, &projection)? {
                    records.push(record);
                }
                if records.len() == AT_ONCE {
                    write(assets, &scope, &library, town, std::mem::take(&mut records))?;
                }
            }
            write(assets, &scope, &library, town, records)?;
            keep(assets, &key, &reader.digest, next.saturating_sub(1))?;
        }
    }
    // What is kept and read no more: its reader no longer reads the
    // project, or the project is gone; nothing of a reader gone with it.
    for (key, (_, through)) in kept {
        let (account, opened, town) = &key;
        if let Some(opened) = accounts.get(&(account.clone(), *opened))
            && let (Ok(name), Ok(town)) = (AccountName::try_from(account.as_str()), TownId::try_from(town.clone()))
            && let Some((scope, library)) = told::scope_of(assets, &name, *opened)?
        {
            let gone: Vec<_> = (1..=through)
                .filter_map(|seq| {
                    let item = seq.to_be_bytes().to_vec();
                    let held = library.item(&scope.descriptor.scope, &stream(&town), &item).ok().flatten();
                    held.map(|_| KindRecord {
                        kind: ProjectLedger::KIND,
                        schema: ProjectLedger::SCHEMA,
                        record: Item { item: item.clone(), order: item, state: State::Gone },
                    })
                })
                .collect();
            for part in gone.chunks(AT_ONCE) {
                write(assets, &scope, &library, &town, part.to_vec())?;
            }
        }
        forget(assets, &key)?;
    }
    Ok(())
}

/// The record of `projection`, when the library does not hold it as it is.
fn record(
    scope: &scopes::Scope,
    library: &fastctx_content::v2::Library,
    town: &TownId,
    projection: &fastctx_net::ledger::Projection,
) -> Result<Option<KindRecord>, String> {
    let bytes = serde_json::to_vec(projection).map_err(|error| error.to_string())?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let body = serde_json::to_vec(&serde_json::json!({ "seq": projection.seq, "size": bytes.len(), "sha256": digest }))
        .map_err(|error| error.to_string())?;
    let item = projection.seq.to_be_bytes().to_vec();
    let error = |error: fastctx_content::Error| error.to_string();
    if library.item(&scope.descriptor.scope, &stream(town), &item).map_err(error)?.is_some_and(|held| held.body == body) {
        return Ok(None);
    }
    let lease = library.put(&mut &bytes[..], "ledger", LEASE).map_err(error)?;
    Ok(Some(KindRecord {
        kind: ProjectLedger::KIND,
        schema: ProjectLedger::SCHEMA,
        record: Item { item: item.clone(), order: item, state: State::Is { body, objects: vec![lease.object] } },
    }))
}

/// Writes `records` into `town`'s stream of the reader's records.
fn write(
    assets: &Assets,
    scope: &scopes::Scope,
    library: &fastctx_content::v2::Library,
    town: &TownId,
    records: Vec<KindRecord>,
) -> Result<(), String> {
    let _writing = super::mirror::writing(&scope.descriptor.scope);
    super::mirror::apply(assets, scope, library, &stream(town), records)
}

fn kept(assets: &Assets) -> Result<Kept, String> {
    assets
        .control
        .read_now(|c| {
            let mut statement = c.prepare_cached("SELECT account, opened, town, machines, through FROM asset_ledger_views")?;
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
