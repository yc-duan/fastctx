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

/// Whether the record `kind` `key` is one of the conversation `resident`'s
/// documents: its plan, or a page of its history.
pub(crate) fn documents_of(kind: &str, key: &str, resident: &str) -> bool {
    (kind == fastctx_types::plan::KIND && key == resident)
        || (kind == fastctx_types::history::RECORD_KIND
            && key.strip_prefix(resident).is_some_and(|suffix| suffix.starts_with('/')))
}

/// Lets go of each revision of the conversation `resident`'s documents
/// staged in `slot` and never published, as the conversation is erased
/// (`crate::owned_records::deletion::erase`): its text is on the disk
/// whether the asset library took it or not (codex-d515b194). A file
/// published of one the account's index does not keep, `indexed` saying
/// which it does, goes too, with the revisions kept before it. Only what
/// `owns` proves the conversation's goes. One staged that cannot be read is
/// not known to be the conversation's, and stays. Says what went and what
/// stayed.
pub(crate) fn let_go_of(
    slot: &Slot,
    resident: &str,
    indexed: &std::collections::BTreeSet<(String, String)>,
    owns: &dyn Fn(&fastctx_types::account_records::Metadata) -> bool,
) -> std::io::Result<LetGo> {
    let files = match fastctx_platform::disk::read_dir(slot.root()) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(LetGo::default()),
        Err(error) => return Err(error),
    };
    let gone_if_there = |path: &Path| match fastctx_platform::disk::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    let mut let_go = LetGo::default();
    for file in files {
        let staged = file?.path();
        if !is_staged(&staged) {
            continue;
        }
        let bytes = fastctx_platform::disk::read(&staged)?;
        let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
            let_go.unread += 1;
            continue;
        };
        let (kind, key) = (&record.metadata.kind, &record.metadata.key);
        if !documents_of(kind, key, resident) {
            continue;
        }
        if path(slot, kind, key) != staged {
            let_go.unread += 1;
            continue;
        }
        if !owns(&record.metadata) {
            let_go.foreign += 1;
            continue;
        }
        if !indexed.contains(&(kind.clone(), key.clone())) {
            gone_if_there(&slot.path(kind, key))?;
        }
        super::history::delete(&slot.history(kind, key))?;
        gone_if_there(&staged)?;
        let_go.gone += 1;
    }
    Ok(let_go)
}

/// What `let_go_of` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LetGo {
    /// The conversation's revisions that went.
    pub(crate) gone: usize,
    /// Of its id, not proven its own: they stay.
    pub(crate) foreign: usize,
    /// Staged revisions read as nothing's, or not where they say: they stay.
    pub(crate) unread: usize,
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

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_types::account_records::Metadata;

    fn plan(resident: &str) -> (Record, Vec<u8>) {
        let record = Record {
            metadata: Metadata {
                kind: fastctx_types::plan::KIND.into(),
                key: resident.into(),
                version: 1,
                by: None,
                at: jiff::Timestamp::UNIX_EPOCH,
                deleted: false,
            },
            body: serde_json::json!({ "text": "what only its conversation knows" }),
        };
        let bytes = serde_json::to_vec(&record).expect("json");
        (record, bytes)
    }

    #[test]
    fn a_conversations_staged_text_goes_with_it() {
        let dir = super::super::storage::tests::Scratch::new("staged");
        let directory: fastctx_net::directory::Directory =
            serde_json::from_value(serde_json::json!({"accounts": [], "members": [], "projects": []}))
                .expect("a directory");
        let records = super::super::storage::Records::open(&dir.0, &directory).expect("records");
        let slot = records.account(&"carol".parse().expect("an account"), jiff::Timestamp::UNIX_EPOCH);
        let kind = fastctx_types::plan::KIND;
        let (_, gone) = plan("r-0a1b");
        let (_, stays) = plan("r-2c3d");
        stage(&path(&slot, kind, "r-0a1b"), &gone).expect("staged");
        stage(&path(&slot, kind, "r-2c3d"), &stays).expect("staged");
        // Published as the library took it, the index not following.
        fastctx_platform::disk::write(slot.path(kind, "r-0a1b"), &gone).expect("written");
        let let_go = let_go_of(&slot, "r-0a1b", &std::collections::BTreeSet::new(), &|_| true).expect("let go of");
        assert_eq!(let_go.gone, 1);
        assert!(!fastctx_platform::disk::is_there(path(&slot, kind, "r-0a1b")), "its staged text goes");
        assert!(!fastctx_platform::disk::is_there(slot.path(kind, "r-0a1b")), "its file the index does not keep goes");
        assert!(fastctx_platform::disk::is_there(path(&slot, kind, "r-2c3d")), "another conversation's stays");
        // One the index keeps is as the deletion left it.
        stage(&path(&slot, kind, "r-0a1b"), &gone).expect("staged");
        fastctx_platform::disk::write(slot.path(kind, "r-0a1b"), b"tombstone").expect("written");
        let indexed = std::collections::BTreeSet::from([(kind.to_string(), "r-0a1b".to_string())]);
        assert_eq!(let_go_of(&slot, "r-0a1b", &indexed, &|_| true).expect("let go of").gone, 1);
        assert!(fastctx_platform::disk::is_there(slot.path(kind, "r-0a1b")), "the file the index keeps stays");
        // One not proven its own, and one read as nothing's, stay, counted.
        stage(&path(&slot, kind, "r-0a1b"), &gone).expect("staged");
        fastctx_platform::disk::write(slot.root().join("junk.staged"), b"{").expect("written");
        let counted = let_go_of(&slot, "r-0a1b", &indexed, &|_| false).expect("let go of");
        assert_eq!((counted.gone, counted.foreign, counted.unread), (0, 1, 1));
        assert!(fastctx_platform::disk::is_there(path(&slot, kind, "r-0a1b")), "not proven its own, it stays");
    }
}
