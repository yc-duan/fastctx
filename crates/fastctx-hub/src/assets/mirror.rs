//! The hub's account records of a conversation's plan and notes written
//! again into the asset library (design §28.17): as the hub accepts a write
//! of one, the record goes into a stream the hub orders in the
//! conversation's scope — the plan into `plan`, each note by its name into
//! `history` — from where every device holding the conversation takes it as
//! a replica. The account records stay where these are written and merged.
//! One not yet in a scope the hub described is written once it is.

use super::Assets;
use super::scopes;
use fastctx_content::v2::{ConversationHistory, ConversationPlan, Header, Kind as _, KindRecord, Received, Signer, Taking, Verifier};
use fastctx_content::{ObjectRef, Record as Item, State};
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_types::account_records::Record;
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
        PublicKey::from_bytes(header.key).verify(Domain::SyncSourceBatch, header_bytes, &Signature::from_bytes(signature)).is_ok()
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
                .query_row(fastctx_storage::rusqlite::params![ending, ending.len() as i64], |row| row.get::<_, String>(0))
                .optional()?)
        })
        .map_err(|error| error.to_string())
}

/// The version of the account record a document's item holds.
fn version_of(body: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(body).ok()?.get("metadata")?.get("version")?.as_u64()
}

/// Writes `record`, a plan's or a conversation note's account record the
/// hub accepted, into the stream the hub orders in its conversation's scope.
/// Call on a thread that may block.
pub(crate) fn document(assets: &Assets, record: &Record) -> Result<(), String> {
    let (kind, key) = (record.metadata.kind.as_str(), record.metadata.key.as_str());
    let (resident, stream, item, number) = if kind == fastctx_types::plan::KIND {
        (key, ConversationPlan::STREAM, "plan", ConversationPlan::KIND)
    } else if kind == fastctx_types::history::RECORD_KIND {
        let Some((resident, name)) = key.split_once('/') else { return Ok(()) };
        (resident, ConversationHistory::STREAM, name, ConversationHistory::KIND)
    } else {
        return Ok(());
    };
    let Some(scope_key) = conversation_scope(assets, resident)? else { return Ok(()) };
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
    let next = library.streams(&scope_key).map_err(error)?.into_iter().find(|(name, _)| name == stream).map_or(0, |(_, next)| next);
    assets.moved_by_hub(&scope.owner, &scope_key, stream, next);
    Ok(())
}
