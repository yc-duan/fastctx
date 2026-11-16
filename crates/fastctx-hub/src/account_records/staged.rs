//! A record whose place is in the asset library is written there first
//! (design §15 `dual`); the file the hub keeps it in is its projection. The
//! new revision is staged beside that file before the library takes it, so a
//! hub stopped between the two publishes it as it next opens: the library
//! holding that revision proves it was accepted, and a staged revision the
//! library lacks was never answered and is dropped.

use std::path::{Path, PathBuf};

use fastctx_types::account_records::Record;

use super::storage::{Account, Entry, Slot};

/// Where the revision of `kind`/`key` in `slot` is staged.
pub(crate) fn path(slot: &Slot, kind: &str, key: &str) -> PathBuf {
    slot.path(kind, key).with_extension("staged")
}

/// Whether `path` is a staged revision.
pub(crate) fn is_staged(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "staged")
}

/// Stages `bytes`, the revision about to be taken into the library.
pub(crate) fn stage(staged: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fastctx_platform::ensure_private_dir(staged.parent().expect("record folder"))?;
    fastctx_platform::publish(staged, bytes, fastctx_platform::Permissions::Private)
}

/// Publishes the staged revision `bytes` of `record` as the file the hub
/// keeps it in: the revision before it preserved first where revisions are
/// kept, and the kept revisions gone with a deleted record.
pub(crate) fn publish(
    live: &Path,
    history: Option<&Path>,
    staged: &Path,
    bytes: &[u8],
    record: &Record,
) -> std::io::Result<()> {
    if let Some(history) = history {
        super::history::preserve(live, history)?;
    }
    fastctx_platform::publish(live, bytes, fastctx_platform::Permissions::Private)?;
    if record.metadata.deleted
        && let Some(history) = history
    {
        super::history::delete(history)?;
    }
    match fastctx_platform::disk::remove_file(staged) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Whether revisions of `kind` are kept beside the record.
pub(crate) fn archived(kind: &str) -> bool {
    matches!(kind, fastctx_types::plan::KIND | fastctx_types::history::RECORD_KIND)
}

/// What became of a staged revision as the hub opened.
pub(crate) enum Settled {
    Published,
    Dropped,
    /// The library could not say whether it took it: left staged.
    Unknown(String),
}

/// Settles a revision staged in `slot` as the hub stopped: published when
/// `taken` (the library holds it, or a later one), dropped when it does not.
/// The account's index follows what is published.
pub(crate) fn settle(
    slot: &Slot,
    account: &mut Account,
    staged: &Path,
    taken: impl FnOnce(&Record) -> Result<bool, String>,
) -> Result<Settled, String> {
    let bytes = fastctx_platform::disk::read(staged).map_err(|error| error.to_string())?;
    let record: Record = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let (kind, key) = (record.metadata.kind.clone(), record.metadata.key.clone());
    if path(slot, &kind, &key) != staged {
        return Err(format!("unexpected staged record path {}", staged.display()));
    }
    let identity = (kind.clone(), key.clone());
    let current = account.entries.get(&identity).map(|entry| entry.metadata.version);
    let behind = current.is_some_and(|version| version >= record.metadata.version);
    let drop = behind
        || match taken(&record) {
            Ok(taken) => !taken,
            Err(why) => return Ok(Settled::Unknown(why)),
        };
    if drop {
        fastctx_platform::disk::remove_file(staged).map_err(|error| error.to_string())?;
        return Ok(Settled::Dropped);
    }
    let live = slot.path(&kind, &key);
    let history = archived(&kind).then(|| slot.history(&kind, &key));
    publish(&live, history.as_deref(), staged, &bytes, &record).map_err(|error| error.to_string())?;
    let before = account.entries.get(&identity).map_or(0, |entry| entry.size);
    account.bytes = account.bytes - before + bytes.len();
    account.entries.insert(
        identity,
        Entry {
            metadata: record.metadata.clone(),
            size: bytes.len(),
            history: super::storage::history_head(&record).map_err(|error| error.to_string())?,
            conversation: super::storage::record_conversation(&record),
        },
    );
    Ok(Settled::Published)
}
