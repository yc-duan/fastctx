//! The hub's account records written again into the asset library (design
//! §28.17): as the hub accepts a write of one, the record goes into a stream
//! the hub orders — a conversation's plan, notes, goals and sentinels in the
//! conversation's scope, an account's skills and settings in its records'
//! scope — from where every device holding that scope takes it as a
//! replica. The account records stay where these are written and merged.
//! Those of a scope described after they were written are written as it is
//! described, and all of them once after the hub starts.

use super::Assets;
use super::scopes;
use fastctx_content::v2::{
    AccountSettings, AccountSkill, ConversationGoal, ConversationHistory, ConversationPlan, ConversationSentinel,
    Header, Kind as _, KindRecord, LEASE, ProjectLedger, Received, Signer, Taking, Verifier,
};
use fastctx_content::{ObjectRef, Record as Item, State};
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_types::AccountName;
use fastctx_types::access::TownId;
use fastctx_types::account_records::Record;
use fastctx_types::assets::Readers;
use std::collections::BTreeSet;
use std::sync::{Condvar, LazyLock, Mutex, PoisonError};

/// The scopes the hub is writing into now: its writes into one scope are
/// made one at a time, those into others meanwhile (AR13).
static WRITING: LazyLock<(Mutex<BTreeSet<String>>, Condvar)> = LazyLock::new(Default::default);

/// The hub writing into one scope: let go as it is dropped.
pub(super) struct Writing(String);

impl Drop for Writing {
    fn drop(&mut self) {
        let (held, freed) = &*WRITING;
        held.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.0);
        freed.notify_all();
    }
}

/// Waits until no other write into `scope` is being made, and holds it.
pub(super) fn writing(scope: &str) -> Writing {
    let (held, freed) = &*WRITING;
    let mut writing = held.lock().unwrap_or_else(PoisonError::into_inner);
    while writing.contains(scope) {
        writing = freed.wait(writing).unwrap_or_else(PoisonError::into_inner);
    }
    writing.insert(scope.to_string());
    Writing(scope.to_string())
}

/// The longest body written as it is; a record larger is its own object,
/// its body saying so (AR03).
const BODY_MOST: usize = fastctx_content::RECORD_BYTES - 4096;

/// An item's identity for `name`: as it is when it fits, otherwise a digest
/// of it, the whole name staying in the body (AR03).
pub(super) fn item_of(name: &str) -> Vec<u8> {
    if name.len() <= fastctx_content::KEY_BYTES {
        return name.as_bytes().to_vec();
    }
    use sha2::Digest as _;
    let digest: String = sha2::Sha256::digest(name.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("#{}", &digest[..fastctx_content::KEY_BYTES - 1]).into_bytes()
}

/// A body as it goes in a record, the objects it refers to, and the lease
/// that keeps a new one until the record is taken.
pub(super) type Fitted = (Vec<u8>, Vec<ObjectRef>, Option<fastctx_content::v2::Lease>);

/// `body` as it goes in a record: as it is, or, larger than a body holds,
/// as its one object with what is said of it — its metadata — in the body.
pub(super) fn fitted(library: &fastctx_content::v2::Library, body: Vec<u8>) -> Result<Fitted, String> {
    if body.len() <= BODY_MOST {
        return Ok((body, Vec::new(), None));
    }
    let metadata = serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|value| value.get("metadata").cloned());
    let lease = library.put(&mut body.as_slice(), "mirror", LEASE).map_err(|error| error.to_string())?;
    let mut said = serde_json::json!({ "in_object": true, "bytes": body.len() });
    if let Some(metadata) = metadata {
        said["metadata"] = metadata;
    }
    let said = serde_json::to_vec(&said).map_err(|error| error.to_string())?;
    Ok((said, vec![lease.object], Some(lease)))
}

/// The hub's service key, signing frames as the stream's ordering device.
struct Service<'a>(&'a super::keys::Keys);

impl Signer for Service<'_> {
    fn device(&self) -> [u8; 32] {
        self.0.frame_key()
    }

    fn key(&self) -> [u8; 32] {
        self.0.frame_key()
    }

    fn sign(&self, header: &[u8]) -> Vec<u8> {
        self.0.sign_frame(header)
    }
}

/// Checks the hub's own frames as a device's are checked.
struct Keyed;

impl Verifier for Keyed {
    fn verify(&self, header: &Header, header_bytes: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = <[u8; 64]>::try_from(signature) else { return false };
        PublicKey::from_bytes(header.key)
            .verify(Domain::SyncSourceBatch, header_bytes, &Signature::from_bytes(signature))
            .is_ok()
    }
}

/// The scope of the conversation `resident` of `account` (opened at
/// `opened`), as the hub described it: one that account opened and owns,
/// never another's that ends the same (AR01).
fn conversation_scope(
    assets: &Assets,
    account: &AccountName,
    opened: jiff::Timestamp,
    resident: &str,
) -> Result<Option<String>, String> {
    let ending = format!("/{resident}");
    let keys: Vec<String> = assets
        .control
        .read_now(move |c| {
            let mut statement = c.prepare_cached("SELECT key FROM asset_scopes WHERE substr(key, -?2) = ?1 ORDER BY key LIMIT 64")?;
            let rows = statement.query_map(fastctx_storage::rusqlite::params![ending, ending.len() as i64], |row| {
                row.get::<_, String>(0)
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(|error| error.to_string())?;
    for key in keys {
        let Some(scope) = scopes::scope(assets, &key)? else { continue };
        // The account's own, or a body's it opened on a machine shared whole
        // (§6.2), the account then as it was opened when described.
        let ours = match (&scope.owner, &scope.descriptor.readers) {
            (super::owners::Owner::Account(name, at), Readers::Conversation { openers, .. }) => {
                name == account.as_str() && *at == opened.as_millisecond() && openers.contains(account)
            }
            (super::owners::Owner::Body(_), Readers::Body { opener, .. }) => {
                opener == account
                    && scope.descriptor.born.get(&format!("account:{account}")) == Some(&opened.as_millisecond().to_string())
            }
            _ => false,
        };
        if ours {
            return Ok(Some(key));
        }
    }
    Ok(None)
}

/// The version of the account record a document's item holds.
fn version_of(body: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(body).ok()?.get("metadata")?.get("version")?.as_u64()
}

/// The key of the scope an account's records are kept in on this hub: by
/// when it was opened too, so one made again under the name has its own
/// (AR04, NN05).
pub(crate) fn account_scope(assets: &Assets, account: &AccountName, opened: jiff::Timestamp) -> String {
    fastctx_types::assets::records_key(assets.id.as_str(), account, opened.as_millisecond())
}

/// Where a record goes: the conversation it is of, or none for the
/// account's own; its stream, its item and its kind's number. None for a
/// record that does not go into the asset library.
fn placed(record: &Record) -> Option<(Option<String>, &'static str, String, u16)> {
    let (kind, key) = (record.metadata.kind.as_str(), record.metadata.key.as_str());
    Some(match kind {
        fastctx_types::plan::KIND => {
            (Some(key.to_string()), ConversationPlan::STREAM, "plan".into(), ConversationPlan::KIND)
        }
        fastctx_types::history::RECORD_KIND => {
            let (resident, name) = key.split_once('/')?;
            (Some(resident.to_string()), ConversationHistory::STREAM, name.into(), ConversationHistory::KIND)
        }
        "goal" | "goal_past" => (Some(key.to_string()), ConversationGoal::STREAM, kind.into(), ConversationGoal::KIND),
        "sentinel" => {
            let resident = record.body["definition"]["owner"]["resident"].as_str()?;
            (Some(resident.to_string()), ConversationSentinel::STREAM, key.into(), ConversationSentinel::KIND)
        }
        fastctx_types::skills::KIND | fastctx_types::skills::INDEX_KIND => {
            (None, AccountSkill::STREAM, format!("{kind}/{key}"), AccountSkill::KIND)
        }
        fastctx_types::sentinel_settings::RECORD_KIND | fastctx_types::placement::KIND => {
            (None, AccountSettings::STREAM, format!("{kind}/{key}"), AccountSettings::KIND)
        }
        _ => return None,
    })
}

/// Whether `record`'s kind goes into the asset library.
pub(crate) fn mirrored(kind: &str) -> bool {
    matches!(
        kind,
        fastctx_types::plan::KIND
            | fastctx_types::history::RECORD_KIND
            | "goal"
            | "goal_past"
            | "sentinel"
            | fastctx_types::skills::KIND
            | fastctx_types::skills::INDEX_KIND
            | fastctx_types::sentinel_settings::RECORD_KIND
            | fastctx_types::placement::KIND
    )
}

/// Writes `record`, an account record of `account` the hub accepted, into
/// the stream the hub orders in the scope it is of. Call on a thread that
/// may block.
pub(crate) fn document(
    assets: &Assets,
    account: &AccountName,
    opened: jiff::Timestamp,
    record: &Record,
) -> Result<(), String> {
    let Some((resident, stream, item, number)) = placed(record) else { return Ok(()) };
    let item = item_of(&item);
    let scope_key = match &resident {
        Some(resident) => match conversation_scope(assets, account, opened, resident)? {
            Some(key) => key,
            None => return Ok(()),
        },
        None => account_scope(assets, account, opened),
    };
    let Some(scope) = scopes::scope(assets, &scope_key)? else { return Ok(()) };
    let library = scopes::library(assets, &scope)?;
    let error = |error: fastctx_content::Error| error.to_string();
    let _writing = writing(&scope_key);
    // This version or a later one written already: nothing. A deleted
    // record stays an item with its version, so a late older write cannot
    // bring it back (AR02).
    if let Some(entry) = library.item(&scope_key, stream, &item).map_err(error)?
        && version_of(&entry.body).is_some_and(|version| version >= record.metadata.version)
    {
        return Ok(());
    }
    // The lease keeps a new object until the record is taken, for as long
    // as leases last.
    let (body, objects, _lease) = fitted(&library, serde_json::to_vec(record).map_err(|error| error.to_string())?)?;
    let records = vec![KindRecord {
        kind: number,
        schema: 1,
        record: Item { item: item.clone(), order: item, state: State::Is { body, objects } },
    }];
    apply(assets, &scope, &library, stream, records)
}

/// Takes `records`, which the hub writes into `stream` of `scope`, into the
/// scope's library as frames the hub orders, and tells the devices holding
/// it. Called with the scope's writing held (`writing`).
pub(super) fn apply(
    assets: &Assets,
    scope: &scopes::Scope,
    library: &fastctx_content::v2::Library,
    stream: &str,
    records: Vec<KindRecord>,
) -> Result<(), String> {
    if records.is_empty() {
        return Ok(());
    }
    // A sealed hub writes nothing new until it is opened again (AR08);
    // what is left is written then.
    if let Some(why) = assets.sealed() {
        return Err(format!("the hub is sealed: {why}"));
    }
    let scope_key = scope.descriptor.scope.as_str();
    let error = |error: fastctx_content::Error| error.to_string();
    let frames = library.hub_frames(scope_key, stream, records, &Service(assets.keys())).map_err(error)?;
    let period = library.holding(scope_key).map_err(error)?.ok_or("the scope is not held here")?;
    let anything = |_: &ObjectRef| true;
    let nothing = |_: &[u8; 32]| false;
    let taking = Taking { period, readable: &anything, proven: &nothing };
    for frame in frames {
        match library.take(scope_key, frame, &Keyed, &taking, super::serve::kinds(), assets.signer()).map_err(error)? {
            Received::Applied { .. } => {}
            _ => return Err(format!("the hub did not apply its own frame of {stream} in {scope_key}")),
        }
    }
    assets.changed(scope.owner.clone(), scope_key);
    let next = library
        .streams(scope_key)
        .map_err(error)?
        .into_iter()
        .find(|(name, _)| name == stream)
        .map_or(0, |(_, next)| next);
    assets.moved_by_hub(&scope.owner, scope_key, stream, next);
    Ok(())
}


/// The records of `account` (opened at `opened`) that `wanted` picks, read
/// from where the hub keeps them.
async fn records_of(
    hub: &crate::Hub,
    account: &AccountName,
    opened: jiff::Timestamp,
    wanted: impl Fn(&crate::account_records::storage::Entry) -> bool,
) -> Vec<std::path::PathBuf> {
    let slot = hub.account_records.account(account, opened);
    let records = slot.records.clone().lock_owned().await;
    records
        .entries
        .values()
        .filter(|entry| mirrored(&entry.metadata.kind) && wanted(entry))
        .map(|entry| slot.path(&entry.metadata.kind, &entry.metadata.key))
        .collect()
}

/// The records whose writing into the asset library failed — a full disk,
/// a sealed hub — tried again every minute while the hub runs (AR15); after
/// it starts, everything is looked at anyway.
type Failed = BTreeSet<(String, i64, std::path::PathBuf)>;
static FAILED: LazyLock<Mutex<Failed>> = LazyLock::new(Mutex::default);

/// Writing the record at `path` of `account` failed: tried again.
pub(crate) fn failed(account: &AccountName, opened: jiff::Timestamp, path: std::path::PathBuf) {
    FAILED.lock().unwrap_or_else(PoisonError::into_inner).insert((account.to_string(), opened.as_millisecond(), path));
}

/// Tries again what failed to be written, and what the hub tells accounts
/// that failed (`told`).
pub(crate) async fn again(hub: std::sync::Arc<crate::Hub>) {
    let failed = std::mem::take(&mut *FAILED.lock().unwrap_or_else(PoisonError::into_inner));
    let mut by_account: std::collections::BTreeMap<(String, i64), Vec<std::path::PathBuf>> = Default::default();
    for (account, opened, path) in failed {
        by_account.entry((account, opened)).or_default().push(path);
    }
    for ((account, opened), paths) in by_account {
        let (Ok(account), Ok(opened)) = (AccountName::try_from(account), jiff::Timestamp::from_millisecond(opened)) else {
            continue;
        };
        write_all(&hub, account, opened, paths).await;
    }
    super::told::again(&hub).await;
}

/// Writes each record at `paths`, of `account`, into the asset library.
async fn write_all(
    hub: &std::sync::Arc<crate::Hub>,
    account: AccountName,
    opened: jiff::Timestamp,
    paths: Vec<std::path::PathBuf>,
) {
    if paths.is_empty() {
        return;
    }
    let (assets, log) = (hub.assets.clone(), hub.log.clone());
    let _ = fastctx_platform::task::spawn_blocking(move || {
        for path in paths {
            let written = crate::account_records::storage::read(&path)
                .and_then(|record| document(&assets, &account, opened, &record));
            if let Err(error) = written {
                log.line(format_args!("writing an account record of {account} into the asset library: {error}"));
                failed(&account, opened, path);
            }
        }
    })
    .await;
}

/// The scopes `keys` were described just now: what the hub's records hold
/// of them is written into them.
pub(crate) async fn described(hub: std::sync::Arc<crate::Hub>, keys: Vec<String>) {
    for key in keys {
        // A project's ledger scope: what its ledger holds goes in.
        if let Some(town) = key
            .strip_prefix("p/")
            .and_then(|rest| rest.strip_suffix("/ledger"))
            .and_then(|town| TownId::try_from(town.to_string()).ok())
        {
            ledger_of(hub.clone(), town).await;
            continue;
        }
        let Ok(Some(scope)) = scopes::scope(&hub.assets, &key) else { continue };
        let super::owners::Owner::Account(name, opened) = &scope.owner else { continue };
        let (Ok(account), Ok(opened)) =
            (AccountName::try_from(name.as_str()), jiff::Timestamp::from_millisecond(*opened))
        else {
            continue;
        };
        let paths = if key == account_scope(&hub.assets, &account, opened) {
            super::told::caught_up(&hub, &account, opened).await;
            records_of(&hub, &account, opened, placed_account).await
        } else {
            let resident = key.rsplit('/').next().unwrap_or_default().to_string();
            records_of(&hub, &account, opened, |entry| of_conversation(entry, &resident)).await
        };
        write_all(&hub, account, opened, paths).await;
    }
}

/// Everything the hub's records hold that goes into the asset library,
/// written once after the hub starts: only what the library lacks is.
pub(crate) async fn everything(hub: std::sync::Arc<crate::Hub>) {
    let accounts: Vec<(AccountName, jiff::Timestamp)> =
        hub.store().book.directory.accounts.iter().map(|owner| (owner.name.clone(), owner.opened)).collect();
    for (account, opened) in accounts {
        let paths = records_of(&hub, &account, opened, |_| true).await;
        write_all(&hub, account, opened, paths).await;
    }
    let towns: Vec<TownId> = hub.ledgers.lock().expect("never poisoned").towns.keys().cloned().collect();
    for town in towns {
        ledger_of(hub.clone(), town).await;
    }
}

/// Whether `entry` is one of an account's own records that go into its
/// records' scope.
fn placed_account(entry: &crate::account_records::storage::Entry) -> bool {
    matches!(
        entry.metadata.kind.as_str(),
        fastctx_types::skills::KIND
            | fastctx_types::skills::INDEX_KIND
            | fastctx_types::sentinel_settings::RECORD_KIND
            | fastctx_types::placement::KIND
    )
}

/// Whether `entry` is of the conversation `resident`.
fn of_conversation(entry: &crate::account_records::storage::Entry, resident: &str) -> bool {
    let key = entry.metadata.key.as_str();
    match entry.metadata.kind.as_str() {
        fastctx_types::plan::KIND | "goal" | "goal_past" => key == resident,
        fastctx_types::history::RECORD_KIND => key.split_once('/').is_some_and(|(of, _)| of == resident),
        "sentinel" => entry.conversation.as_deref() == Some(resident),
        _ => false,
    }
}

/// The key of the scope a project's ledger is kept in.
pub(crate) fn project_scope(town: &TownId) -> String {
    format!("{}ledger", Readers::Project { town: town.clone() }.key_prefix(""))
}

/// Writes the entry `seq` of `town`'s ledger, its signed bytes `bytes`, into
/// the stream the hub orders in the project's scope (design §28.18), once
/// the scope is described. Call on a thread that may block.
pub(crate) fn ledger_entry(assets: &Assets, town: &TownId, seq: u64, bytes: &[u8]) -> Result<(), String> {
    let scope_key = project_scope(town);
    let Some(scope) = scopes::scope(assets, &scope_key)? else { return Ok(()) };
    let library = scopes::library(assets, &scope)?;
    let error = |error: fastctx_content::Error| error.to_string();
    let _writing = writing(&scope_key);
    let item = seq.to_be_bytes().to_vec();
    if library.item(&scope_key, ProjectLedger::STREAM, &item).map_err(error)?.is_some() {
        return Ok(());
    }
    let lease = library.put(&mut &bytes[..], "ledger", LEASE).map_err(error)?;
    let body = serde_json::to_vec(&serde_json::json!({ "seq": seq, "size": bytes.len() }))
        .map_err(|error| error.to_string())?;
    let records = vec![KindRecord {
        kind: ProjectLedger::KIND,
        schema: 1,
        record: Item { item: item.clone(), order: item, state: State::Is { body, objects: vec![lease.object] } },
    }];
    apply(assets, &scope, &library, ProjectLedger::STREAM, records)
}

/// Writes what of `town`'s ledger the asset library lacks, in order.
pub(crate) async fn ledger_of(hub: std::sync::Arc<crate::Hub>, town: TownId) {
    let (assets, log) = (hub.assets.clone(), hub.log.clone());
    let next = hub.ledgers.lock().expect("never poisoned").towns.get(&town).map_or(1, |ledger| ledger.next);
    let paths: Vec<(u64, std::path::PathBuf)> = {
        let ledgers = hub.ledgers.lock().expect("never poisoned");
        (1..next).map(|seq| (seq, ledgers.entry_path(&town, seq))).collect()
    };
    let _ = fastctx_platform::task::spawn_blocking(move || {
        for (seq, path) in paths {
            let Ok(bytes) = fastctx_platform::disk::read(&path) else { continue };
            if let Err(error) = ledger_entry(&assets, &town, seq, &bytes) {
                log.line(format_args!("writing entry {seq} of {town} into the asset library: {error}"));
                return;
            }
        }
    })
    .await;
}
