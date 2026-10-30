//! Composer metadata, text journals, and operation receipts commit together.
//! Rows live with the conversation; its records expire as `keeping` says.

mod text;
use text::TextWrite;
pub(super) use text::prepare as prepare_text;

use super::*;
use fastctx_storage::rusqlite::{Connection, OptionalExtension, params};
use fastctx_storage::{Error, Result as StoredResult, documents};

// Keep the owner index separate from large legacy character-alias payloads:
// WITHOUT ROWID key comparisons can read overflow records even for existence checks.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS composers (
    owner TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK(revision >= 0),
    content BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS composer_records (
    owner TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    content BLOB NOT NULL,
    expires INTEGER,
    PRIMARY KEY(owner,kind,name),
    FOREIGN KEY(owner) REFERENCES composers(owner) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS composer_expiry ON composer_records(expires) WHERE expires IS NOT NULL;
"#;

fn error(reason: impl std::fmt::Display) -> Error {
    Error::new(reason.to_string())
}
fn key(owner: &str, name: &str) -> documents::Key {
    documents::Key { namespace: "composer".into(), owner: owner.into(), name: name.into() }
}

pub(super) async fn exists(node: &Node, id: &ResidentId) -> Result<bool, Failure> {
    let owner = id.to_string();
    node.database
        .read(move |connection| {
            Ok(connection
                .query_row("SELECT 1 FROM composers WHERE owner=?1", [&owner], |_| Ok(()))
                .optional()?
                .is_some())
        })
        .await
        .map_err(failure)
}

pub(super) async fn read(node: &Node, id: &ResidentId) -> Result<Stored, Failure> {
    let owner = id.to_string();
    let directory = node.world.conversation(id);
    node.database
        .read(move |connection| {
            let transaction = connection.unchecked_transaction()?;
            let row = transaction
                .query_row("SELECT revision,content FROM composers WHERE owner=?1", [&owner], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .optional()?;
            let mut state: Stored = if let Some((revision, bytes)) = row {
                let mut state: Stored = serde_json::from_slice(&bytes).map_err(error)?;
                let revision = u64::try_from(revision).map_err(error)?;
                if state.revision > revision {
                    return Err(error("composer metadata is newer than its durable revision"));
                }
                state.revision = revision;
                state.persisted = true;
                state
            } else {
                // Only the old node wrote this file; the conversation's cell
                // serializes its migration with edits and deletion now.
                match std::fs::read(directory.composer()) {
                    Ok(bytes) => serde_json::from_slice(&bytes).map_err(error)?,
                    Err(reason) if reason.kind() == std::io::ErrorKind::NotFound => Stored::default(),
                    Err(reason) => return Err(error(reason)),
                }
            };
            for document in state.documents_mut() {
                let loaded = documents::load_in(&transaction, &key(&owner, &document.id))?;
                document.hydrate(loaded).map_err(error)?;
            }
            state.document_names = state.documents().map(|document| document.id.clone()).collect();
            state.reindex();
            state.observed_documents = state.visible_document_ids();
            transaction.commit()?;
            Ok(state)
        })
        .await
        .map_err(failure)
}

struct Record {
    kind: &'static str,
    name: String,
    content: Vec<u8>,
    /// When it may go, in milliseconds (`keeping`).
    expires: Option<i64>,
}
pub(super) struct Prepared {
    owner: String,
    previous: Option<u64>,
    revision: u64,
    content: Vec<u8>,
    texts: Vec<TextWrite>,
    deleted: Vec<documents::Key>,
    records: Vec<Record>,
    pub bytes: usize,
}

pub(super) fn prepare(id: &ResidentId, state: &mut Stored) -> Result<Prepared, Failure> {
    let owner = id.to_string();
    let previous = state.persisted.then_some(state.revision);
    // Old draft files contain receipts without a retirement proof. Importing
    // them does not establish that their devices can no longer replay them.
    let persisted = state.persisted;
    let expires = |kind| persisted.then(|| super::keeping::expires(kind));
    state.revision = state.revision.checked_add(1).ok_or_else(|| failure("revision overflow"))?;
    i64::try_from(state.revision).map_err(failure)?;
    let mut texts = Vec::new();
    for document in state.documents_mut() {
        if !document.pending.is_empty() || document.revision == 0 {
            texts.push(TextWrite::prepare(&owner, document)?);
        }
    }
    state.reindex();
    let names: HashSet<_> = state.documents().map(|document| document.id.clone()).collect();
    let deleted: Vec<_> = state.document_names.difference(&names).map(|name| key(&owner, name)).collect();
    state.document_names = names;
    let mut records = Vec::new();
    for (name, receipt) in std::mem::take(&mut state.receipts) {
        records.push(Record {
            kind: "receipts",
            name: receipts::name(&name),
            content: serde_json::to_vec(&receipt).map_err(failure)?,
            expires: expires("receipts"),
        });
    }
    for (name, operations) in std::mem::take(&mut state.taken) {
        records.push(Record {
            kind: "taken",
            name: receipts::name(&name),
            content: serde_json::to_vec(&operations).map_err(failure)?,
            expires: expires("taken"),
        });
    }
    for name in std::mem::take(&mut state.ended) {
        records.push(Record {
            kind: "ended",
            name: receipts::name(&name),
            content: b"true".to_vec(),
            expires: expires("ended"),
        });
    }
    let finished: Vec<_> =
        state.uploads.iter().filter(|(_, record)| record.finished()).map(|(key, _)| key.clone()).collect();
    for name in finished {
        let record = state.uploads.remove(&name).expect("finished upload");
        records.push(Record {
            kind: "uploads",
            name: receipts::name(&name),
            content: serde_json::to_vec(&record).map_err(failure)?,
            expires: expires("uploads"),
        });
    }
    state.persisted = true;
    let content = serde_json::to_vec(&state).map_err(failure)?;
    let bytes = owner.len()
        + content.len()
        + texts.iter().map(TextWrite::bytes).sum::<usize>()
        + deleted.iter().map(documents::Key::bytes).sum::<usize>()
        + records.iter().map(|record| record.name.len() + record.content.len()).sum::<usize>();
    Ok(Prepared { owner, previous, revision: state.revision, content, texts, deleted, records, bytes })
}

impl Prepared {
    pub fn updates(&self) -> Result<Vec<fastctx_types::composer::sync::Update>, Failure> {
        self.texts.iter().map(TextWrite::update).collect()
    }

    pub fn commit(self, transaction: &Connection) -> StoredResult<()> {
        let revision = i64::try_from(self.revision).map_err(error)?;
        if let Some(previous) = self.previous {
            let changed = transaction.execute(
                "UPDATE composers SET revision=?2,content=?3 WHERE owner=?1 AND revision=?4",
                params![self.owner, revision, self.content, i64::try_from(previous).map_err(error)?],
            )?;
            if changed != 1 {
                return Err(error("the composer changed before its update could commit; read it again"));
            }
        } else {
            transaction.execute(
                "INSERT INTO composers(owner,revision,content) VALUES(?1,?2,?3)",
                params![self.owner, revision, self.content],
            )?;
        }
        for text in self.texts {
            text.commit(transaction)?;
        }
        for key in self.deleted {
            documents::delete(transaction, &key)?;
        }
        for record in self.records {
            transaction.execute(
                "INSERT INTO composer_records(owner,kind,name,content,expires) VALUES(?1,?2,?3,?4,?5) \
                 ON CONFLICT(owner,kind,name) DO UPDATE SET content=excluded.content,expires=excluded.expires",
                params![self.owner, record.kind, record.name, record.content, record.expires],
            )?;
        }
        Ok(())
    }
}

pub(super) async fn delete(node: &Node, id: &ResidentId) -> Result<(), Failure> {
    let owner = id.to_string();
    node.database
        .write(owner.len(), move |transaction| {
            documents::delete_owner(transaction, "composer", &owner)?;
            transaction.execute("DELETE FROM composers WHERE owner=?1", [&owner])?;
            Ok(())
        })
        .await
        .map_err(failure)
}
