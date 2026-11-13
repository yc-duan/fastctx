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
    Header, Kind as _, KindRecord, Received, Signer, Taking, Verifier,
};
use fastctx_content::{ObjectRef, Record as Item, State};
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_types::AccountName;
use fastctx_types::account_records::Record;
use fastctx_types::assets::Readers;
use std::sync::{Mutex, PoisonError};

/// The hub's writes into the streams it orders, one at a time.
static WRITING: Mutex<()> = Mutex::new(());

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

/// The scope of the conversation `resident`, as the hub described it.
fn conversation_scope(assets: &Assets, resident: &str) -> Result<Option<String>, String> {
    let ending = format!("/{resident}");
    assets
        .control
        .read_now(move |c| {
            use fastctx_storage::rusqlite::OptionalExtension as _;
            Ok(c.prepare_cached("SELECT key FROM asset_scopes WHERE substr(key, -?2) = ?1 ORDER BY key LIMIT 1")?
                .query_row(fastctx_storage::rusqlite::params![ending, ending.len() as i64], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?)
        })
        .map_err(|error| error.to_string())
}

/// The version of the account record a document's item holds.
fn version_of(body: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(body).ok()?.get("metadata")?.get("version")?.as_u64()
}

/// The key of the scope an account's records are kept in on this hub.
pub(crate) fn account_scope(assets: &Assets, account: &AccountName) -> String {
    format!("{}records", Readers::Account { account: account.clone() }.key_prefix(assets.id.as_str()))
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
pub(crate) fn document(assets: &Assets, account: &AccountName, record: &Record) -> Result<(), String> {
    let Some((resident, stream, item, number)) = placed(record) else { return Ok(()) };
    let item = item.as_str();
    let scope_key = match &resident {
        Some(resident) => match conversation_scope(assets, resident)? {
            Some(key) => key,
            None => return Ok(()),
        },
        None => account_scope(assets, account),
    };
    let Some(scope) = scopes::scope(assets, &scope_key)? else { return Ok(()) };
    let library = scopes::library(assets, &scope)?;
    let error = |error: fastctx_content::Error| error.to_string();
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    // This version or a later one written already: nothing.
    if let Some(entry) = library.item(&scope_key, stream, item.as_bytes()).map_err(error)?
        && version_of(&entry.body).is_some_and(|version| version >= record.metadata.version)
    {
        return Ok(());
    }
    let state = if record.metadata.deleted {
        State::Gone
    } else {
        State::Is { body: serde_json::to_vec(record).map_err(|error| error.to_string())?, objects: Vec::new() }
    };
    let records = vec![KindRecord {
        kind: number,
        schema: 1,
        record: Item { item: item.as_bytes().to_vec(), order: item.as_bytes().to_vec(), state },
    }];
    let frames = library.hub_frames(&scope_key, stream, records, &Service(assets.keys())).map_err(error)?;
    let period = library.holding(&scope_key).map_err(error)?.ok_or("the scope is not held here")?;
    let anything = |_: &ObjectRef| true;
    let nothing = |_: &[u8; 32]| false;
    let taking = Taking { period, readable: &anything, proven: &nothing };
    for frame in frames {
        match library.take(&scope_key, frame, &Keyed, &taking, super::serve::kinds(), assets.signer()).map_err(error)? {
            Received::Applied { .. } => {}
            _ => return Err(format!("the hub did not apply its own frame of {stream} in {scope_key}")),
        }
    }
    assets.changed(scope.owner.clone(), &scope_key);
    let next = library
        .streams(&scope_key)
        .map_err(error)?
        .into_iter()
        .find(|(name, _)| name == stream)
        .map_or(0, |(_, next)| next);
    assets.moved_by_hub(&scope.owner, &scope_key, stream, next);
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

/// Writes each record at `paths`, of `account`, into the asset library.
async fn write_all(hub: &std::sync::Arc<crate::Hub>, account: AccountName, paths: Vec<std::path::PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let (assets, log) = (hub.assets.clone(), hub.log.clone());
    let _ = fastctx_platform::task::spawn_blocking(move || {
        for path in paths {
            let written =
                crate::account_records::storage::read(&path).and_then(|record| document(&assets, &account, &record));
            if let Err(error) = written {
                log.line(format_args!("writing an account record of {account} into the asset library: {error}"));
            }
        }
    })
    .await;
}

/// The scopes `keys` were described just now: what the hub's records hold
/// of them is written into them.
pub(crate) async fn described(hub: std::sync::Arc<crate::Hub>, keys: Vec<String>) {
    for key in keys {
        let Ok(Some(scope)) = scopes::scope(&hub.assets, &key) else { continue };
        let super::owners::Owner::Account(name, opened) = &scope.owner else { continue };
        let (Ok(account), Ok(opened)) =
            (AccountName::try_from(name.as_str()), jiff::Timestamp::from_millisecond(*opened))
        else {
            continue;
        };
        let paths = if key == account_scope(&hub.assets, &account) {
            records_of(&hub, &account, opened, placed_account).await
        } else {
            let resident = key.rsplit('/').next().unwrap_or_default().to_string();
            records_of(&hub, &account, opened, |entry| of_conversation(entry, &resident)).await
        };
        write_all(&hub, account, paths).await;
    }
}

/// Everything the hub's records hold that goes into the asset library,
/// written once after the hub starts: only what the library lacks is.
pub(crate) async fn everything(hub: std::sync::Arc<crate::Hub>) {
    let accounts: Vec<(AccountName, jiff::Timestamp)> =
        hub.store().book.directory.accounts.iter().map(|owner| (owner.name.clone(), owner.opened)).collect();
    for (account, opened) in accounts {
        let paths = records_of(&hub, &account, opened, |_| true).await;
        write_all(&hub, account, paths).await;
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
